// The ask card in its compact form (Home's "Waiting for you") and the way a
// time ask hands its amount to the lever sheet.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import type { Child, ChildRequest } from "../domain/types";

const ctx = {
  state: { activity: [], signer: { connected: true, kind: "none", autoSign: false } },
  approveRequest: vi.fn(),
  denyRequest: vi.fn(),
  approveAppOpen: vi.fn(),
  dismissRequest: vi.fn(),
  setRequestChosenMinutes: vi.fn(),
  deviceStatus: {},
};

vi.mock("../store/store", async (orig) => ({
  ...(await orig<typeof import("../store/store")>()),
  useCharter: () => ctx,
}));

import { RequestCard } from "./RequestCard";

const CHILD: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) }],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};

const TIME_ASK: ChildRequest = {
  id: "r1",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "time.extend",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks for 15 more minutes",
  minutesRequested: 15,
  status: "pending",
};

const APP_ASK: ChildRequest = {
  id: "r2",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "app.open",
  appId: "com.mojang.minecraftpe",
  appLabel: "Minecraft",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks to open Minecraft",
  status: "pending",
};

describe("RequestCard", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (ui: React.ReactElement) => {
    act(() => {
      root.render(ui);
    });
  };
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${container.textContent}`);
    return b;
  };
  const click = async (b: HTMLButtonElement) => {
    await act(async () => {
      b.click();
    });
  };

  beforeEach(() => {
    for (const k of ["approveRequest", "denyRequest", "approveAppOpen", "dismissRequest", "setRequestChosenMinutes"] as const) {
      ctx[k].mockReset();
    }
    ctx.approveRequest.mockResolvedValue(undefined);
    ctx.denyRequest.mockResolvedValue(undefined);
    ctx.approveAppOpen.mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("compact: one line, and one tap approves the amount asked", async () => {
    mount(<RequestCard request={TIME_ASK} child={CHILD} compact />);
    expect(container.textContent).toContain("Sam wants 15 more minutes");
    // The full card's furniture is behind "Details".
    expect(container.textContent).not.toContain("From Sam's laptop");
    await click(button("Approve 15 min"));
    expect(ctx.approveRequest).toHaveBeenCalledTimes(1);
    expect(ctx.approveRequest).toHaveBeenCalledWith("r1", 15);
  });

  it("compact: 'Not now' denies in one tap", async () => {
    mount(<RequestCard request={TIME_ASK} child={CHILD} compact />);
    await click(button("Not now"));
    expect(ctx.denyRequest).toHaveBeenCalledTimes(1);
    expect(ctx.denyRequest).toHaveBeenCalledWith("r1");
  });

  it("compact app ask: '1 hour' is the one-tap answer, and Details shows every window", async () => {
    mount(<RequestCard request={APP_ASK} child={CHILD} compact />);
    expect(container.textContent).toContain("Sam: open Minecraft?");
    await click(button("1 hour"));
    expect(ctx.approveAppOpen).toHaveBeenCalledTimes(1);
    const [id, until, window] = ctx.approveAppOpen.mock.calls[0];
    expect(id).toBe("r2");
    expect(window).toBe("1h");
    expect(until).toBeGreaterThan(Date.now() / 1000 + 3000);

    await click(container.querySelector<HTMLButtonElement>(".request-compact-head")!);
    expect(container.textContent).toContain("Open for…");
    expect(button("30 min")).toBeDefined();
  });

  it("only one decision is in flight across cards: the second ask's answers wait for the first", async () => {
    const other: ChildRequest = {
      ...APP_ASK,
      id: "r3",
      appId: "com.google.android.youtube",
      appLabel: "YouTube",
    };
    let release!: () => void;
    ctx.approveAppOpen.mockImplementationOnce(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    mount(
      <>
        <RequestCard request={APP_ASK} child={CHILD} compact />
        <RequestCard request={other} child={CHILD} compact />
      </>,
    );
    const [first, second] = [...container.querySelectorAll<HTMLButtonElement>("button")].filter(
      (b) => b.textContent?.trim() === "1 hour",
    );
    await click(first);
    expect(ctx.approveAppOpen).toHaveBeenCalledTimes(1);
    // The second card's answers are disabled, and tapping it anyway does nothing.
    expect(second.disabled).toBe(true);
    await click(second);
    expect(ctx.approveAppOpen).toHaveBeenCalledTimes(1);

    await act(async () => {
      release();
    });
    expect(second.disabled).toBe(false);
    await click(second);
    expect(ctx.approveAppOpen).toHaveBeenCalledTimes(2);
    expect(ctx.approveAppOpen.mock.calls[1][0]).toBe("r3");
  });

  it("a send the signer never answers stops blocking the other asks after a minute", async () => {
    vi.useFakeTimers();
    try {
      const other: ChildRequest = { ...APP_ASK, id: "r3", appLabel: "YouTube" };
      ctx.approveAppOpen.mockImplementationOnce(() => new Promise<void>(() => {}));
      mount(
        <>
          <RequestCard request={APP_ASK} child={CHILD} compact />
          <RequestCard request={other} child={CHILD} compact />
        </>,
      );
      const [first, second] = [...container.querySelectorAll<HTMLButtonElement>("button")].filter(
        (b) => b.textContent?.trim() === "1 hour",
      );
      await click(first);
      expect(second.disabled).toBe(true);
      await act(async () => {
        vi.advanceTimersByTime(60_000);
      });
      expect(second.disabled).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("full: the amount is chosen in the sheet ('Change amount'), not by an inline stepper", async () => {
    mount(<RequestCard request={TIME_ASK} child={CHILD} />);
    expect(container.querySelector('[aria-label="5 minutes less"]')).toBeNull();
    expect(button("Approve 15 min")).toBeDefined();

    await click(button("Change amount"));
    expect(container.querySelector(".sheet-backdrop")).not.toBeNull();
    await click(button("−"));
    expect(ctx.setRequestChosenMinutes).toHaveBeenCalledWith("r1", 10);
  });

  it("an expired time ask offers Give time instead, which opens the give sheet", async () => {
    const stale = { ...TIME_ASK, createdAt: Date.now() - 30 * 3600 * 1000 };
    mount(<RequestCard request={stale} child={CHILD} />);
    expect(container.textContent).toContain("This ask expired on the device");
    expect(button("Expired").disabled).toBe(true);
    await click(button("Give Sam time instead"));
    expect(container.textContent).toContain("Give Sam more time");
    expect(button("+15 min")).toBeDefined();
  });

  it("Dismiss signs nothing and is a full-size target", async () => {
    mount(<RequestCard request={TIME_ASK} child={CHILD} />);
    await click(container.querySelector<HTMLButtonElement>('button[aria-label="Dismiss"]')!);
    expect(ctx.dismissRequest).toHaveBeenCalledWith("r1");
    expect(ctx.denyRequest).not.toHaveBeenCalled();
  });
});
