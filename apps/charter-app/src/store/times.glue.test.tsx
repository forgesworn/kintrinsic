// Times, through the real provider with only the signer faked: the four pieces
// of glue the domain layer could not reach.
//   1. `savePolicy` signs `times` and keeps them locally (and a later save that
//      never mentions times neither re-signs nor loses them);
//   2. a gift carries the kind and the devices the lever chose;
//   3. an approval's GRANT echoes the ask's own `time`;
//   4. answering with another kind of time hands the signer the ask's wire
//      correlation (with its `time`) and closes the ask.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const fake = vi.hoisted(() => ({
  signClause: vi.fn(),
  signGiftClause: vi.fn(),
  signDecision: vi.fn(),
  answerAskWithOtherTime: vi.fn(),
}));

vi.mock("../signer/selectSigner", () => ({
  selectSigner: () => ({
    status: () => ({ connected: true, kind: "local", autoSign: true }),
    connect: async () => {},
    disconnect: () => {},
    setAutoSign: async () => {},
    signClause: fake.signClause,
    signGiftClause: fake.signGiftClause,
    signDecision: fake.signDecision,
    answerAskWithOtherTime: fake.answerAskWithOtherTime,
  }),
}));

import { CharterProvider, STORAGE_KEY, useCharter, type CharterContextValue, type CharterState } from "./store";
import type { Child, ChildRequest, NamedTime } from "../domain/types";

const LAPTOP = "a".repeat(64);
const PHONE = "b".repeat(64);

const LEARNING: NamedTime = {
  id: "learning",
  label: "Learning time",
  posture: "onlyThese",
  apps: ["org.example.maths"],
};

const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
    { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
  ],
  policies: [
    {
      id: "p1",
      scope: { kind: "device" },
      schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
    },
  ],
};

const ASK: ChildRequest = {
  id: "r1",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "time.extend",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks for 30 more minutes",
  minutesRequested: 30,
  limitHit: "schedule",
  time: "learning",
  status: "pending",
  reqId: "f".repeat(64),
  nonce: "1".repeat(64),
  machine: LAPTOP,
  subject: "c".repeat(64),
};

describe("times glue in the store", () => {
  let container: HTMLDivElement;
  let root: Root;
  let api: CharterContextValue;

  function Probe() {
    api = useCharter();
    return null;
  }

  const stored = () => JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;

  beforeEach(() => {
    for (const f of Object.values(fake)) f.mockReset();
    fake.signClause.mockResolvedValue({ wraps: [] });
    fake.signGiftClause.mockResolvedValue({ wraps: [] });
    fake.signDecision.mockResolvedValue({});
    fake.answerAskWithOtherTime.mockResolvedValue({ wraps: [] });
    const state: CharterState = {
      children: [SAM],
      requests: [ASK],
      activity: [],
      signer: { connected: true, kind: "local", autoSign: true },
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
  });

  it("savePolicy signs the times and keeps them; a later save without times keeps them and signs none", async () => {
    await act(async () => {
      await api.savePolicy("c1", "p1", { times: [LEARNING] });
    });
    expect(fake.signClause).toHaveBeenCalledTimes(1);
    const signed = fake.signClause.mock.calls[0][1];
    expect(signed.times).toEqual([LEARNING]);
    expect(stored().children[0].policies[0].times).toEqual([LEARNING]);
    expect(stored().activity[0].summary).toBe("You updated their times");

    await act(async () => {
      await api.savePolicy("c1", "p1", { budget: { tz: "UTC", dailyMinutes: 60 } });
    });
    // The second save never mentioned times: nothing times-shaped is signed...
    expect(fake.signClause.mock.calls[1][1].times).toBeUndefined();
    // ...and the saved copy still has them.
    expect(stored().children[0].policies[0].times).toEqual([LEARNING]);
  });

  it("an empty set is a real value: deleting the last time is signed and kept as []", async () => {
    await act(async () => {
      await api.savePolicy("c1", "p1", { times: [LEARNING] });
      await api.savePolicy("c1", "p1", { times: [] });
    });
    expect(fake.signClause.mock.calls[1][1].times).toEqual([]);
    expect(stored().children[0].policies[0].times).toEqual([]);
  });

  it("giveTime hands the signer the kind and the devices, and leaves the old call alone without them", async () => {
    await act(async () => {
      await api.savePolicy("c1", "p1", { times: [LEARNING] });
    });
    await act(async () => {
      await api.giveTime("c1", 30, undefined, { time: "learning", timeLabel: "Learning time", deviceIds: ["d1"] });
    });
    expect(fake.signGiftClause).toHaveBeenLastCalledWith("c1", 30, expect.anything(), undefined, {
      time: "learning",
      timeLabel: "Learning time",
      deviceIds: ["d1"],
    });
    expect(stored().activity[0].summary).toContain("of Learning time on Sam's laptop");

    // A time id the guardian never defined is never written into the log raw.
    await act(async () => {
      await api.giveTime("c1", 30, undefined, { time: "<script>", timeLabel: "<script>", deviceIds: ["d1"] });
    });
    expect(stored().activity[0].summary).toContain("of another kind of time");
    expect(stored().activity[0].summary).not.toContain("<script>");

    await act(async () => {
      await api.giveTime("c1", 15);
    });
    expect(fake.signGiftClause.mock.calls.at(-1)?.[4]).toBeUndefined();
    expect(stored().activity[0].summary).toBe("You gave 15 more minutes");
  });

  it("approving a typed ask echoes its time in the grant's wire block; an untyped ask has none", async () => {
    await act(async () => {
      await api.approveRequest("r1", 30);
    });
    expect(fake.signDecision.mock.calls[0][2].wire).toMatchObject({ machine: LAPTOP, time: "learning" });

    const untyped: ChildRequest = { ...ASK, id: "r2", reqId: "e".repeat(64), time: undefined };
    const state = stored();
    state.requests = [untyped];
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => root.unmount());
    root = createRoot(container);
    act(() => {
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
    await act(async () => {
      await api.approveRequest("r2", 30);
    });
    expect("time" in fake.signDecision.mock.calls[1][2].wire).toBe(false);
  });

  it("answering with another kind of time sends the ask's wire block plus the kind, and closes the ask", async () => {
    await act(async () => {
      await api.answerRequestWithOtherTime("r1", 20, undefined, "Free time");
    });
    const call = fake.answerAskWithOtherTime.mock.calls[0];
    expect(call[0]).toBe("r1");
    expect(call[1].wire).toMatchObject({ machine: LAPTOP, time: "learning" });
    expect(call.slice(2)).toEqual([20, undefined, "Free time"]);
    expect(stored().requests[0].status).toBe("approved");
    expect(stored().activity[0].summary).toContain("20 min of Free time instead");
  });

  it("refuses to answer a simulated ask (nothing on the wire) rather than record a yes that sent nothing", async () => {
    const local: ChildRequest = { ...ASK, id: "r3", reqId: undefined, nonce: undefined, machine: undefined, subject: undefined };
    const state = stored();
    state.requests = [local];
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => root.unmount());
    root = createRoot(container);
    act(() => {
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
    await expect(api.answerRequestWithOtherTime("r3", 20, undefined)).rejects.toThrow(/parent approval/);
    expect(fake.answerAskWithOtherTime).not.toHaveBeenCalled();
    expect(stored().requests[0].status).toBe("pending");
  });
});
