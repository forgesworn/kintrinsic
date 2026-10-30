// More time when the child has times: the kind picker, the device picker, their
// defaults, and what reaches the store. (The no-times sheet is covered by
// LeverSheet.test.tsx, which must keep passing unchanged.)
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import type { Child, ChildRequest, NamedTime } from "../domain/types";

const MON_10 = Date.UTC(2026, 9, 26, 10, 0, 0);
const ctx = {
  giveTime: vi.fn(),
  standDown: vi.fn(),
  approveRequest: vi.fn(),
  denyRequest: vi.fn(),
  setRequestChosenMinutes: vi.fn(),
  answerRequestWithOtherTime: vi.fn(),
  deviceStatus: {} as Record<string, unknown>,
};

vi.mock("../store/store", async (orig) => ({
  ...(await orig<typeof import("../store/store")>()),
  useCharter: () => ctx,
}));

import { LeverSheet } from "./LeverSheet";

const LAPTOP = "a".repeat(64);
const PHONE = "b".repeat(64);
const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", apps: ["org.example.maths"] };
const READING: NamedTime = { id: "reading", label: "Reading time", posture: "onlyThese", apps: ["org.example.books"] };

function sam(times: NamedTime[] | undefined, devices?: Child["devices"]): Child {
  return {
    id: "c1",
    name: "Sam",
    color: "#3B6FB2",
    dependantPubkey: null,
    devices: devices ?? [
      { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
      { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
    ],
    policies: [
      {
        id: "p1",
        scope: { kind: "device" },
        times,
        // Learning on the laptop on weekday mornings; the phone is just Free.
        schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } },
        deviceOverrides: {
          d1: {
            schedule: {
              tz: "UTC",
              weekly: { mon: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }] },
            },
          },
          d2: { schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } } },
        },
      },
    ],
  };
}

const status = (locked: boolean) => ({
  v: 1,
  subject: "c",
  machine: "m",
  ts: MON_10 / 1000,
  dayKey: "2026-10-26",
  usedTodaySecs: 0,
  windowLeftSecs: 0,
  quotaLeftSecs: 0,
  effectiveSecs: 0,
  locked,
  source: "schedule",
});

describe("More time with times", () => {
  let container: HTMLDivElement;
  let root: Root;
  const onClose = vi.fn();

  const mount = (ui: React.ReactElement) => act(() => root.render(ui));
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${container.textContent}`);
    return b;
  };
  const click = async (b: HTMLElement) => {
    await act(async () => {
      b.click();
    });
  };
  const select = (id: string) => container.querySelector<HTMLSelectElement>(`select[id^="${id}"]`)!;
  const boxes = () => [...container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')];
  const choose = async (sel: HTMLSelectElement, value: string) => {
    await act(async () => {
      sel.value = value;
      sel.dispatchEvent(new Event("change", { bubbles: true }));
    });
  };

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(MON_10);
    for (const fn of Object.values(ctx)) if (typeof fn === "function") (fn as ReturnType<typeof vi.fn>).mockReset();
    ctx.deviceStatus = {};
    ctx.giveTime.mockResolvedValue(true);
    ctx.answerRequestWithOtherTime.mockResolvedValue(undefined);
    onClose.mockReset();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("offers no kind picker to a child with no times, and no device list to a child with one device", () => {
    mount(<LeverSheet mode="give" child={sam(undefined, [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP }])} onClose={onClose} />);
    expect(container.querySelector("select")).toBeNull();
    expect(boxes()).toHaveLength(0);
  });

  it("opens on the time the laptop is in, and a preset gives that kind", async () => {
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    // 10:00 on Monday: the laptop is inside its Learning window.
    expect(select("give-time").value).toBe("learning");
    await click(button("+30 min"));
    expect(ctx.giveTime).toHaveBeenCalledTimes(1);
    const [childId, minutes, group, opts] = ctx.giveTime.mock.calls[0];
    expect([childId, minutes, group]).toEqual(["c1", 30, undefined]);
    expect(opts).toMatchObject({ time: "learning", timeLabel: "Learning time" });
    expect(onClose).toHaveBeenCalledWith("30 minutes of Learning time sent to Sam.");
  });

  it("aims at the locked devices by default, and only those", async () => {
    ctx.deviceStatus = { [LAPTOP]: status(true), [PHONE]: status(false) };
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    expect(boxes().map((b) => b.checked)).toEqual([true, false]);
    await click(button("+15 min"));
    expect(ctx.giveTime.mock.calls[0][3]).toMatchObject({ deviceIds: ["d1"] });
  });

  it("aims at every device when none is locked, and then sends the old call", async () => {
    ctx.deviceStatus = { [LAPTOP]: status(false), [PHONE]: status(false) };
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    expect(boxes().map((b) => b.checked)).toEqual([true, true]);
    await choose(select("give-time"), "");
    await click(button("+15 min"));
    // Free time, everyone: exactly the call this sheet always made.
    expect(ctx.giveTime).toHaveBeenCalledWith("c1", 15, undefined);
  });

  it("lets the guardian change the devices", async () => {
    ctx.deviceStatus = { [LAPTOP]: status(true), [PHONE]: status(false) };
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    await click(boxes()[1]);
    expect(boxes().map((b) => b.checked)).toEqual([true, true]);
    await click(boxes()[0]);
    await click(button("+30 min"));
    expect(ctx.giveTime.mock.calls[0][3]).toMatchObject({ time: "learning", deviceIds: ["d2"] });
  });

  it("will not give a typed kind to a device too old to run times, and says why", async () => {
    // The phone reports an Android build below 49.
    ctx.deviceStatus = { [PHONE]: { ...status(false), appVersionCode: 40 } };
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    expect(boxes()[1].disabled).toBe(true);
    expect(boxes()[1].checked).toBe(false);
    expect(container.textContent).toContain("Needs updating to run Learning time");
    await click(button("+15 min"));
    expect(ctx.giveTime.mock.calls[0][3]).toMatchObject({ time: "learning", deviceIds: ["d1"] });
  });

  it("with nobody chosen, nothing can be sent", async () => {
    mount(<LeverSheet mode="give" child={sam([LEARNING])} onClose={onClose} />);
    for (const b of boxes()) if (b.checked) await click(b);
    expect(container.textContent).toContain("Choose a device to give it to.");
    expect(button("+30 min").disabled).toBe(true);
    expect(button("Give 30 min of Learning time").disabled).toBe(true);
  });

  it("hides the group picker for a typed gift (a gift tops up one pool)", async () => {
    const child = sam([LEARNING, READING]);
    child.policies[0].buckets = {
      enabled: true,
      tz: "UTC",
      buckets: [{ id: "play", label: "Play", apps: ["x"], dailyMinutes: 60 }],
    };
    mount(<LeverSheet mode="give" child={child} onClose={onClose} />);
    expect(container.textContent).not.toContain("Add it to");
    await choose(select("give-time"), "");
    expect(container.textContent).toContain("Add it to");
  });

  describe("answering an ask with a different kind", () => {
    const ASK: ChildRequest = {
      id: "r1",
      childId: "c1",
      requester: "device",
      deviceId: "d1",
      kind: "time.extend",
      createdAt: MON_10 - 60_000,
      title: "Sam's laptop asks for 30 more minutes",
      minutesRequested: 30,
      time: "learning",
      status: "pending",
      reqId: "f".repeat(64),
      nonce: "1".repeat(64),
      machine: LAPTOP,
      subject: "c".repeat(64),
    };

    it("offers every kind except the one asked for, and answers with the chosen one", async () => {
      mount(<LeverSheet mode="retype" child={sam([LEARNING, READING])} request={ASK} onClose={onClose} />);
      const options = [...container.querySelectorAll("option")].map((o) => o.textContent);
      expect(options).toEqual(["Free time", "Reading time"]);
      await click(button("Give 30 min of Free time"));
      expect(ctx.answerRequestWithOtherTime).toHaveBeenCalledWith("r1", 30, undefined, "Free time");
      expect(onClose).toHaveBeenCalledWith("30 minutes of Free time sent instead.");
    });

    it("passes a named kind's id and label", async () => {
      mount(<LeverSheet mode="retype" child={sam([LEARNING, READING])} request={ASK} onClose={onClose} />);
      await choose(select("retype"), "reading");
      await click(button("Give 30 min of Reading time"));
      expect(ctx.answerRequestWithOtherTime).toHaveBeenCalledWith("r1", 30, "reading", "Reading time");
    });

    it("shows the error and stays open when it cannot be sent", async () => {
      ctx.answerRequestWithOtherTime.mockRejectedValue(new Error("Turn on parent approval first, then try again."));
      mount(<LeverSheet mode="retype" child={sam([LEARNING])} request={ASK} onClose={onClose} />);
      await click(button("Give 30 min of Free time"));
      expect(container.querySelector('[role="alert"]')?.textContent).toBe("Turn on parent approval first, then try again.");
      expect(onClose).not.toHaveBeenCalled();
    });
  });

  it("the approve sheet says which kind the ask is for", () => {
    mount(
      <LeverSheet
        mode="approve"
        child={sam([LEARNING])}
        request={{
          id: "r1",
          childId: "c1",
          requester: "device",
          kind: "time.extend",
          createdAt: MON_10 - 1000,
          title: "t",
          minutesRequested: 30,
          time: "learning",
          status: "pending",
        }}
        onClose={onClose}
      />,
    );
    expect(container.textContent).toContain("This is for Learning time.");
  });
});
