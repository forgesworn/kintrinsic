// A typed ask: the card says which kind of time was asked for, approves it as
// asked (the grant echoes `time`), or offers a different kind.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import type { Child, ChildRequest, NamedTime } from "../domain/types";

const ctx = {
  state: { activity: [], signer: { connected: true, kind: "none", autoSign: false } },
  approveRequest: vi.fn(),
  denyRequest: vi.fn(),
  approveAppOpen: vi.fn(),
  dismissRequest: vi.fn(),
  setRequestChosenMinutes: vi.fn(),
  answerRequestWithOtherTime: vi.fn(),
  giveTime: vi.fn(),
  deviceStatus: {},
};

vi.mock("../store/store", async (orig) => ({
  ...(await orig<typeof import("../store/store")>()),
  useCharter: () => ctx,
}));

import { RequestCard } from "./RequestCard";

const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", apps: ["x"] };

const child = (times?: NamedTime[]): Child => ({
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) }],
  policies: [{ id: "p1", scope: { kind: "device" }, times }],
});

const ask = (over: Partial<ChildRequest> = {}): ChildRequest => ({
  id: "r1",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "time.extend",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks for 30 more minutes",
  minutesRequested: 30,
  limitHit: "schedule",
  status: "pending",
  reqId: "f".repeat(64),
  nonce: "1".repeat(64),
  machine: "a".repeat(64),
  subject: "c".repeat(64),
  ...over,
});

describe("RequestCard for a typed ask", () => {
  let container: HTMLDivElement;
  let root: Root;
  const mount = (ui: React.ReactElement) => act(() => root.render(ui));
  const text = () => container.textContent ?? "";
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${text()}`);
    return b;
  };
  const click = async (b: HTMLElement) => {
    await act(async () => {
      b.click();
    });
  };

  beforeEach(() => {
    for (const k of ["approveRequest", "denyRequest", "answerRequestWithOtherTime", "giveTime"] as const) ctx[k].mockReset();
    ctx.approveRequest.mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("names the kind in the title, the sentence and the approve button", () => {
    mount(<RequestCard request={ask({ time: "learning" })} child={child([LEARNING])} />);
    expect(text()).toContain("More Learning time?");
    expect(text()).toContain("Sam asks for 30 min of Learning time on Sam's laptop.");
    expect(button("Approve 30 min of Learning time")).toBeTruthy();
  });

  it("approves as asked: the store gets the amount, and the grant echoes the ask's own time via its wire block", async () => {
    mount(<RequestCard request={ask({ time: "learning" })} child={child([LEARNING])} />);
    await click(button("Approve 30 min of Learning time"));
    expect(ctx.approveRequest).toHaveBeenCalledWith("r1", 30);
  });

  it("the compact line says it too", () => {
    mount(<RequestCard request={ask({ time: "learning" })} child={child([LEARNING])} compact />);
    expect(text()).toContain("Sam wants 30 more minutes of Learning time");
  });

  it("an untyped ask reads exactly as it did", () => {
    mount(<RequestCard request={ask()} child={child([LEARNING])} />);
    expect(text()).toContain("Sam's laptop asks for 30 more minutes");
    expect(text()).not.toContain("More Learning time?");
    expect(button("Approve 30 min")).toBeTruthy();
  });

  it("offers a different kind on a real ask when the child has times, and not otherwise", () => {
    mount(<RequestCard request={ask({ time: "learning" })} child={child([LEARNING])} />);
    expect(button("Give a different kind of time")).toBeTruthy();
    // No times at all and an untyped ask: nothing to switch to.
    mount(<RequestCard request={ask()} child={child(undefined)} />);
    expect(text()).not.toContain("Give a different kind of time");
  });

  it("does not offer it for a simulated ask (nothing on the wire to answer)", () => {
    mount(<RequestCard request={ask({ time: "learning", reqId: undefined })} child={child([LEARNING])} />);
    expect(text()).not.toContain("Give a different kind of time");
  });

  it("opens the sheet that answers with another kind", async () => {
    mount(<RequestCard request={ask({ time: "learning" })} child={child([LEARNING])} />);
    await click(button("Give a different kind of time"));
    expect(text()).toContain("Give Sam instead");
    await click(button("Give 30 min of Free time"));
    expect(ctx.answerRequestWithOtherTime).toHaveBeenCalledWith("r1", 30, undefined, "Free time");
  });

  it("an ask for a time that was deleted cannot be approved as asked, only answered otherwise", () => {
    mount(<RequestCard request={ask({ time: "gone" })} child={child([LEARNING])} />);
    expect(text()).toContain("asked for a kind of time that isn’t set up any more");
    expect(button("Approve 30 min").disabled).toBe(true);
    expect(button("Give a different kind of time").disabled).toBe(false);
  });
});
