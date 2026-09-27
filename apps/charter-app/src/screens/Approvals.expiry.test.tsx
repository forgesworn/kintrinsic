// G-3 (review 2026-09-27): the ward's own broker ages a Pending ask out to
// Expired (terminal) after 24h and refuses to enact a GRANT for one that has
// (7ba043b, 002c211) — the guardian's queue had no matching limit, so a
// guardian could approve (or deny) a 30-hour-old ask, sign and send a real
// decision, and the device would silently ignore it. This proves the
// Approvals screen now shows the expiry and disables both real decisions for
// an ask past that age, while a fresh ask is untouched.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import Approvals from "./Approvals";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";

const CHILD_ID = "child1";
const DEVICE_ID = "dev1";
const NOW = Date.parse("2026-09-27T12:00:00Z");
const DAY_MS = 24 * 3600 * 1000;

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
  policies: [{ id: "pol1", scope: { kind: "device" } }],
};

// 30 hours old — past the ward's own 24h TTL (the exact scenario the review
// names: "a guardian who approves a 30-hour-old ask").
const STALE: ChildRequest = {
  id: "reqStale",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "time.extend",
  createdAt: NOW - 30 * 3600 * 1000,
  title: "Laptop asks for 15 more minutes",
  minutesRequested: 15,
  status: "pending",
};

const FRESH: ChildRequest = {
  id: "reqFresh",
  childId: CHILD_ID,
  requester: "device",
  deviceId: DEVICE_ID,
  kind: "time.extend",
  createdAt: NOW - 60_000,
  title: "Laptop asks for 10 more minutes",
  minutesRequested: 10,
  status: "pending",
};

function seededState(): CharterState {
  return {
    children: [CHILD],
    requests: [STALE, FRESH],
    activity: [],
    signer: { connected: true, kind: "none", autoSign: false },
  };
}

function findButtonByText(container: HTMLElement, text: string): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll("button")).filter(
    (b) => b.textContent?.trim() === text,
  );
}

describe("Approvals — expired asks (G-3)", () => {
  let container: HTMLDivElement;
  let root: Root;
  const realNow = Date.now;

  beforeEach(() => {
    Date.now = () => NOW;
    localStorage.clear();
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seededState()));
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
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
    Date.now = realNow;
  });

  it("shows the expiry notice only on the stale ask", () => {
    expect(container.textContent).toContain("This ask expired on the device");
    // The notice sits on the stale card only — proven indirectly by there
    // being exactly one occurrence of the phrase.
    const count = (container.textContent?.match(/This ask expired on the device/g) ?? []).length;
    expect(count).toBe(1);
  });

  it("disables Approve and Not now on the expired ask, but not on the fresh one", () => {
    const approveButtons = findButtonByText(container, "Expired");
    expect(approveButtons).toHaveLength(1);
    expect(approveButtons[0].disabled).toBe(true);

    const notNowButtons = findButtonByText(container, "Not now");
    expect(notNowButtons).toHaveLength(2);
    // Order follows the newest-first sort: FRESH (createdAt closer to NOW)
    // renders before STALE.
    expect(notNowButtons[0].disabled).toBe(false); // FRESH
    expect(notNowButtons[1].disabled).toBe(true); // STALE

    // The fresh ask still offers a real "Approve N min" — proving the whole
    // screen wasn't disabled, only the expired card.
    const freshApprove = findButtonByText(container, "Approve 10 min");
    expect(freshApprove).toHaveLength(1);
    expect(freshApprove[0].disabled).toBe(false);
  });

  it("the stale ask is exactly at the boundary too — createdAt 24h ago also reads as expired", () => {
    act(() => {
      root.unmount();
    });
    container.remove();

    const boundary: ChildRequest = { ...STALE, id: "reqBoundary", createdAt: NOW - DAY_MS };
    localStorage.clear();
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [CHILD],
        requests: [boundary],
        activity: [],
        signer: { connected: true, kind: "none", autoSign: false },
      } satisfies CharterState),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Approvals />
        </CharterProvider>,
      );
    });
    expect(container.textContent).toContain("This ask expired on the device");
  });
});
