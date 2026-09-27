// G-3 (review 2026-09-27, F3): the ward's own broker ages a Pending ask out
// to Expired (terminal) after 24h and refuses to enact a GRANT for one that
// has (7ba043b, 002c211). Round one only disabled the Approvals screen's
// buttons for a stale ask — the STORE itself (`approveRequest`,
// `approveAppOpen`) had no matching guard, so any other caller (a bug, a
// future surface, a test bypassing the disabled button) could still sign and
// send an approval the device has already stopped listening for. This tests
// the store's own guard directly, bypassing the UI's disabled buttons
// entirely — the exact gap the review named.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { CharterProvider, STORAGE_KEY, useCharter, type CharterState } from "./store";
import type { Child, ChildRequest } from "../domain/types";

const CHILD_ID = "child1";
const DEVICE_ID = "dev1";
const NOW = Date.parse("2026-09-27T12:00:00Z");

const CHILD: Child = {
  id: CHILD_ID,
  name: "Robin",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    {
      id: DEVICE_ID,
      label: "Laptop",
      platform: "linux",
      pairing: "paired",
      devicePubkey: "e".repeat(64),
    },
  ],
  // Deliberately NO `apps` policy: approveAppOpen's OWN "can't find app
  // rules yet" error is used below as a witness that control reached past
  // the expiry guard, without having to build a full apps policy + drive a
  // real signed clause through to completion.
  policies: [{ id: "pol1", scope: { kind: "device" } }],
};

// 30 hours old, wire-correlated — past the ward's own 24h TTL, and a REAL
// device-originated ask (has reqId/nonce/machine), exactly the shape F3
// names.
const STALE_EXTEND: ChildRequest = {
  id: "reqStaleExtend",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "time.extend",
  createdAt: NOW - 30 * 3600 * 1000,
  title: "Laptop asks for 15 more minutes",
  minutesRequested: 15,
  status: "pending",
  reqId: "f".repeat(64),
  nonce: "1".repeat(64),
  machine: "e".repeat(64),
};

const FRESH_EXTEND: ChildRequest = {
  ...STALE_EXTEND,
  id: "reqFreshExtend",
  createdAt: NOW - 60_000,
};

const STALE_APP_OPEN: ChildRequest = {
  id: "reqStaleAppOpen",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "app.open",
  appId: "org.videolan.vlc",
  appLabel: "VLC",
  createdAt: NOW - 30 * 3600 * 1000,
  title: "Laptop asks to open VLC",
  status: "pending",
  reqId: "a".repeat(64),
  nonce: "2".repeat(64),
  machine: "e".repeat(64),
};

const FRESH_APP_OPEN: ChildRequest = {
  ...STALE_APP_OPEN,
  id: "reqFreshAppOpen",
  createdAt: NOW - 60_000,
};

function seededState(requests: ChildRequest[]): CharterState {
  return {
    children: [CHILD],
    requests,
    activity: [],
    // Connected (a real signer, mocked) so a wire-correlated ask's decision
    // path is "sign" rather than the disconnected "needs your signature"
    // refusal — the confirm gate this raises (`pendingSignature`) is a
    // separate, already-tested concern (Approvals.dismiss.test.tsx); here it
    // is only used as a witness that control reached the signer at all.
    signer: { connected: true, kind: "none", autoSign: false },
  };
}

/** Exposes the two store actions under test, plus enough live state to
 *  assert on, alongside the same provider. */
function Probe() {
  const { approveRequest, approveAppOpen, state, pendingSignature } = useCharter();
  return (
    <div>
      <div data-testid="pending-signature">{pendingSignature ? "shown" : "hidden"}</div>
      <div data-testid="statuses">
        {state.requests.map((r) => `${r.id}:${r.status}`).join(",")}
      </div>
      <button
        data-testid="approve-extend"
        onClick={(e) => {
          const id = e.currentTarget.dataset.reqId!;
          approveRequest(id).catch((err: Error) => {
            (window as unknown as Record<string, unknown>).__lastError = err.message;
          });
        }}
      />
      <button
        data-testid="approve-app-open"
        onClick={(e) => {
          const id = e.currentTarget.dataset.reqId!;
          approveAppOpen(id, Math.floor(NOW / 1000) + 600, "30m").catch((err: Error) => {
            (window as unknown as Record<string, unknown>).__lastError = err.message;
          });
        }}
      />
    </div>
  );
}

function click(container: HTMLElement, testId: string, reqId: string) {
  const btn = container.querySelector<HTMLButtonElement>(`[data-testid="${testId}"]`)!;
  btn.dataset.reqId = reqId;
  btn.click();
}

function statuses(container: HTMLElement): string {
  return container.querySelector('[data-testid="statuses"]')?.textContent ?? "";
}

function pendingSignatureText(container: HTMLElement): string | undefined {
  return container.querySelector('[data-testid="pending-signature"]')?.textContent ?? undefined;
}

describe("store expiry guard (G-3/F3): approveRequest and approveAppOpen refuse an expired ask themselves", () => {
  let container: HTMLDivElement;
  let root: Root;
  const realNow = Date.now;

  function mount(requests: ChildRequest[]) {
    localStorage.clear();
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seededState(requests)));
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  }

  beforeEach(() => {
    Date.now = () => NOW;
    delete (window as unknown as Record<string, unknown>).__lastError;
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    localStorage.clear();
    Date.now = realNow;
  });

  it("approveRequest rejects a 30h-old wire-correlated ask, never touches the signer, and leaves it pending", async () => {
    mount([STALE_EXTEND]);
    await act(async () => {
      click(container, "approve-extend", STALE_EXTEND.id);
      await Promise.resolve();
    });
    expect((window as unknown as Record<string, unknown>).__lastError).toMatch(/expired/i);
    expect(pendingSignatureText(container)).toBe("hidden");
    expect(statuses(container)).toBe(`${STALE_EXTEND.id}:pending`);
  });

  it("approveRequest does NOT gate a fresh wire-correlated ask — it reaches the real signer", async () => {
    mount([FRESH_EXTEND]);
    await act(async () => {
      click(container, "approve-extend", FRESH_EXTEND.id);
      await Promise.resolve();
    });
    // No expiry rejection: the guard let it through to `signDecisionIfConnected`,
    // which (connected + mock signer) raises the confirm gate rather than
    // resolving synchronously.
    expect((window as unknown as Record<string, unknown>).__lastError).toBeUndefined();
    expect(pendingSignatureText(container)).toBe("shown");
  });

  it("approveAppOpen rejects a 30h-old wire-correlated app.open ask before ever reaching the apps-policy check", async () => {
    mount([STALE_APP_OPEN]);
    await act(async () => {
      click(container, "approve-app-open", STALE_APP_OPEN.id);
      await Promise.resolve();
    });
    const err = (window as unknown as Record<string, unknown>).__lastError as string;
    expect(err).toMatch(/expired/i);
    expect(err).not.toMatch(/app rules/i);
    expect(statuses(container)).toBe(`${STALE_APP_OPEN.id}:pending`);
  });

  it("approveAppOpen does NOT gate a fresh app.open ask — it reaches the (separate) apps-policy check", async () => {
    mount([FRESH_APP_OPEN]);
    await act(async () => {
      click(container, "approve-app-open", FRESH_APP_OPEN.id);
      await Promise.resolve();
    });
    // CHILD has no `apps` policy at all, so a request that got PAST the
    // expiry guard hits this unrelated, pre-existing error instead — proof
    // the guard did not fire for a fresh ask.
    const err = (window as unknown as Record<string, unknown>).__lastError as string;
    expect(err).toMatch(/app rules/i);
    expect(err).not.toMatch(/expired/i);
  });
});
