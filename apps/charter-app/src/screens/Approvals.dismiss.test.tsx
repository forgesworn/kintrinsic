// Approvals clarity Task C (2026-08-04): "let a repeat ask go without
// answering it" — a small quiet ✕ on each request card.
//
// These are render-level tests (the second ever in this app, after
// `insights/WeeklyPicture.test.tsx` established `react-dom/server` for pure
// components with no context). `Approvals` needs live `useCharter()` state
// (dismiss/deny both mutate it, and the whole point is proving what does and
// doesn't reach the signer), so this mounts the REAL `CharterProvider` in
// jsdom via `react-dom/client` + `react-dom/test-utils`'s `act` — both
// already runtime dependencies, so this adds nothing new.
//
// The signer is seeded CONNECTED but still the mock (`kind: "none"`,
// `autoSign: false`) — every decision must then go through the confirm gate,
// so "did the signer get touched" becomes directly observable as
// `pendingSignature` (exposed here by a sibling `Probe` sharing the same
// provider) flipping non-null. `dismissRequest` must never do that; `Not
// now` (denyRequest) must.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import Approvals from "./Approvals";
import { CharterProvider, STORAGE_KEY, useCharter, type CharterState } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";

const CHILD_ID = "child1";
const DEVICE_ID = "dev1";
const NOW = Date.parse("2026-08-04T12:00:00Z");

const CHILD: Child = {
  id: CHILD_ID,
  name: "Mia",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    {
      id: DEVICE_ID,
      label: "Pixel",
      platform: "android",
      pairing: "paired",
      devicePubkey: "e".repeat(64),
    },
  ],
  policies: [{ id: "pol1", scope: { kind: "device" } }],
};

// A real device-originated ask (carries reqId/nonce/machine) — the exact
// shape whose dedupe-on-reload matters, since a simulated/demo ask has no
// reqId to guard in the first place.
const REQ1: ChildRequest = {
  id: "req1",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "time.extend",
  createdAt: NOW,
  title: "Pixel asks for 15 more minutes",
  minutesRequested: 15,
  status: "pending",
  reqId: "f".repeat(64),
  nonce: "1".repeat(64),
  machine: "e".repeat(64),
};

const REQ2: ChildRequest = {
  id: "req2",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "time.extend",
  createdAt: NOW - 60_000,
  title: "Pixel asks for 10 more minutes",
  minutesRequested: 10,
  status: "pending",
};

function seededState(): CharterState {
  return {
    children: [CHILD],
    requests: [REQ1, REQ2],
    activity: [],
    // connected + mock (not "local") + autoSign off: ANY decide must clear
    // the confirm gate, so a decide that reaches the signer is directly
    // observable as `pendingSignature` going non-null (see file doc).
    signer: { connected: true, kind: "none", autoSign: false },
  };
}

/** Exposes just enough of the live context to assert on, alongside the real
 *  `Approvals` screen mounted under the SAME provider. */
function Probe() {
  const { pendingSignature } = useCharter();
  return <div data-testid="pending-signature">{pendingSignature ? "shown" : "hidden"}</div>;
}

function findDismissButtons(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll<HTMLButtonElement>('button[aria-label="Dismiss"]'));
}

function findButtonByText(container: HTMLElement, text: string): HTMLButtonElement | undefined {
  return Array.from(container.querySelectorAll("button")).find(
    (b) => b.textContent?.trim() === text,
  );
}

function pendingSignatureText(container: HTMLElement): string | undefined {
  return container.querySelector('[data-testid="pending-signature"]')?.textContent ?? undefined;
}

describe("Approvals — dismiss (Task C, approvals-clarity 2026-08-04)", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seededState()));
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Probe />
          <Approvals />
        </CharterProvider>,
      );
    });
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    localStorage.clear();
  });

  it("renders a quiet ✕ Dismiss control on every request card, distinct from Not now", () => {
    const dismissButtons = findDismissButtons(container);
    expect(dismissButtons).toHaveLength(2); // one per pending request

    const notNowButtons = Array.from(container.querySelectorAll("button")).filter(
      (b) => b.textContent?.trim() === "Not now",
    );
    expect(notNowButtons).toHaveLength(2);

    // The two controls are genuinely different elements/labels — a guardian
    // could never mistake one press for the other.
    for (const d of dismissButtons) {
      expect(d.textContent?.trim()).toBe("✕");
      expect(d.getAttribute("aria-label")).toBe("Dismiss");
    }
  });

  it("dismiss removes exactly that request, leaves the other, and never touches the signer", () => {
    expect(container.textContent).toContain("15 more minutes");
    expect(container.textContent).toContain("10 more minutes");
    expect(pendingSignatureText(container)).toBe("hidden");

    const [firstDismiss] = findDismissButtons(container);
    act(() => {
      firstDismiss.click();
    });

    // req1's card is gone; req2's is untouched.
    expect(container.textContent).not.toContain("15 more minutes");
    expect(container.textContent).toContain("10 more minutes");
    expect(findDismissButtons(container)).toHaveLength(1);

    // No signer engagement whatsoever — unlike a real decide (see the next
    // test), dismiss never opens the confirm gate.
    expect(pendingSignatureText(container)).toBe("hidden");

    // And it actually persisted the request's new status — not just removed
    // it from the rendered list.
    const persisted = JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;
    const req1 = persisted.requests.find((r) => r.id === "req1");
    const req2 = persisted.requests.find((r) => r.id === "req2");
    expect(req1?.status).toBe("dismissed");
    expect(req2?.status).toBe("pending");
  });

  it('"Not now" is a real decision and DOES engage the signer — proving dismiss is a genuinely different act', () => {
    expect(pendingSignatureText(container)).toBe("hidden");
    const notNow = findButtonByText(container, "Not now");
    expect(notNow).toBeDefined();
    act(() => {
      notNow!.click();
    });
    // Denying is a real answer: with a connected-but-manual signer, it must
    // raise the confirm gate — the exact thing dismiss (previous test) never
    // does. (The gate is left unresolved deliberately; nothing further needs
    // it for this assertion.)
    expect(pendingSignatureText(container)).toBe("shown");
  });

  it("dismissing survives a reload — the request does not reappear", () => {
    const [firstDismiss] = findDismissButtons(container);
    act(() => {
      firstDismiss.click();
    });
    expect(container.textContent).not.toContain("15 more minutes");

    // Simulate a reload: tear down this mount and mount a FRESH provider
    // reading whatever `loadState()` now finds in localStorage — exactly
    // what happens on an actual page reload.
    act(() => {
      root.unmount();
    });
    container.remove();

    const container2 = document.createElement("div");
    document.body.appendChild(container2);
    let root2!: Root;
    act(() => {
      root2 = createRoot(container2);
      root2.render(
        <CharterProvider>
          <Approvals />
        </CharterProvider>,
      );
    });

    expect(container2.textContent).not.toContain("15 more minutes");
    expect(container2.textContent).toContain("10 more minutes");

    act(() => {
      root2.unmount();
    });
    container2.remove();
  });

  it("is idempotent through the real dismissRequest callback (not just the reducer)", () => {
    function DirectDismiss() {
      const { dismissRequest, state } = useCharter();
      return (
        <div>
          <div data-testid="req1-status">
            {state.requests.find((r) => r.id === "req1")?.status}
          </div>
          <div data-testid="req-count">{state.requests.length}</div>
          <button data-testid="direct-dismiss" onClick={() => dismissRequest("req1")}>
            direct dismiss
          </button>
        </div>
      );
    }

    const c = document.createElement("div");
    document.body.appendChild(c);
    let r!: Root;
    act(() => {
      r = createRoot(c);
      r.render(
        <CharterProvider>
          <DirectDismiss />
        </CharterProvider>,
      );
    });

    const btn = c.querySelector<HTMLButtonElement>('[data-testid="direct-dismiss"]')!;
    act(() => {
      btn.click();
    });
    const afterOnce = c.querySelector('[data-testid="req1-status"]')?.textContent;
    const countAfterOnce = c.querySelector('[data-testid="req-count"]')?.textContent;
    expect(afterOnce).toBe("dismissed");

    act(() => {
      btn.click();
    });
    const afterTwice = c.querySelector('[data-testid="req1-status"]')?.textContent;
    const countAfterTwice = c.querySelector('[data-testid="req-count"]')?.textContent;
    expect(afterTwice).toBe("dismissed");
    expect(countAfterTwice).toBe(countAfterOnce); // no duplicate, nothing added twice

    act(() => {
      r.unmount();
    });
    c.remove();
  });
});
