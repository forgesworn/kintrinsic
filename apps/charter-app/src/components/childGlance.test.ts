import { describe, expect, it } from "vitest";
import type { Child, Device, Policy, Weekday } from "../domain/types";
import { LIVE_WITHIN_MS } from "../domain/liveness";
import type { DeviceStatus } from "../wire/status";
import { childGlance, dailyCap } from "./childGlance";

// Thursday 2 July 2026, 10:00 local time (statusFor reads the local clock).
const NOW = new Date(2026, 6, 2, 10, 0, 0).getTime();
const PUBKEY = "a".repeat(64);

const device = (over: Partial<Device> = {}): Device => ({
  id: "d1",
  label: "Laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: PUBKEY,
  lastSeenAt: NOW - 60_000,
  ...over,
});

const child = (over: Partial<Child> = {}): Child => ({
  id: "c",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [device()],
  policies: [],
  ...over,
});

const capped = (dailyMinutes: number): Policy[] => [
  { id: "p", scope: { kind: "device" }, budget: { tz: "UTC", dailyMinutes } },
];

const WEEKDAYS: Weekday[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
// Open 16:00-18:00 every day, so at 10:00 the child is locked until 4pm.
const afternoonsOnly = (opens = "16:00", closes = "18:00"): Policy[] => [
  {
    id: "p",
    scope: { kind: "device" },
    schedule: {
      tz: "UTC",
      weekly: Object.fromEntries(WEEKDAYS.map((d) => [d, [{ start: opens, end: closes }]])),
    },
  },
];

function feed(over: Partial<DeviceStatus>): DeviceStatus {
  return {
    v: 1,
    subject: "cd".repeat(32),
    machine: PUBKEY,
    ts: NOW / 1000,
    dayKey: "2026-07-02",
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 1800,
    locked: false,
    source: "guardian",
    ...over,
  };
}
const reported = (over: Partial<DeviceStatus>) => ({ [PUBKEY]: feed(over) });

describe("childGlance", () => {
  it("says Set up for a child with no paired device, with no meter", () => {
    const g = childGlance(child({ devices: [] }), {}, NOW);
    expect(g).toEqual({
      line: "Set up",
      tone: "idle",
      fraction: null,
      online: false,
      spoken: "Sam, not set up yet",
    });
    expect(childGlance(child({ devices: [device({ pairing: "pairing" })] }), {}, NOW).line).toBe(
      "Set up",
    );
  });

  it("says No limit when nothing caps the day", () => {
    const g = childGlance(child(), {}, NOW);
    expect(g.line).toBe("No limit");
    expect(g.tone).toBe("ok");
    expect(g.fraction).toBeNull();
    expect(g.spoken).toBe("Sam, online, no time limit today");
  });

  it("says No limit for an unbounded live report too", () => {
    const g = childGlance(child(), reported({ effectiveSecs: 0, source: "unconstrained" }), NOW);
    expect(g.line).toBe("No limit");
  });

  it("shows hours and minutes left, ok, with the fraction of the daily cap", () => {
    const g = childGlance(
      child({ policies: capped(240) }),
      reported({ effectiveSecs: 2 * 3600 + 15 * 60 }),
      NOW,
    );
    expect(g.line).toBe("2h 15m left");
    expect(g.tone).toBe("ok");
    expect(g.fraction).toBeCloseTo(135 / 240);
    expect(g.spoken).toBe("Sam, online, 2 hours 15 minutes left today");
  });

  it("drops the minutes on a whole hour, and the hours under one", () => {
    const left = (secs: number) =>
      childGlance(child(), reported({ effectiveSecs: secs }), NOW).line;
    expect(left(3600)).toBe("1h left");
    expect(left(45 * 60)).toBe("45m left");
    expect(left(60)).toBe("1m left");
  });

  it("turns low under 15 minutes, and not at exactly 15", () => {
    const at = (mins: number) =>
      childGlance(child({ policies: capped(60) }), reported({ effectiveSecs: mins * 60 }), NOW);
    expect(at(14).tone).toBe("low");
    expect(at(14).line).toBe("14m left");
    expect(at(15).tone).toBe("ok");
  });

  it("says <1m left in the last minute, and speaks it plainly", () => {
    const g = childGlance(child({ policies: capped(60) }), reported({ effectiveSecs: 40 }), NOW);
    expect(g.line).toBe("<1m left");
    expect(g.tone).toBe("low");
    expect(g.fraction).toBeCloseTo(40 / 3600);
    expect(g.spoken).toBe("Sam, online, less than a minute left today");
  });

  it("speaks one hour and one minute in the singular", () => {
    const g = childGlance(child(), reported({ effectiveSecs: 3600 + 60 }), NOW);
    expect(g.spoken).toBe("Sam, online, 1 hour 1 minute left today");
  });

  it("counts down from the device's report rather than showing it stale", () => {
    // Reported two minutes ago with 30 minutes then left: 28 left now.
    const g = childGlance(
      child(),
      reported({ ts: NOW / 1000 - 120, effectiveSecs: 1800 }),
      NOW,
    );
    expect(g.line).toBe("28m left");
  });

  it("has no fraction without a daily cap, and clamps one above it", () => {
    expect(childGlance(child(), reported({ effectiveSecs: 600 }), NOW).fraction).toBeNull();
    // A grant can leave more than the base cap.
    const g = childGlance(child({ policies: capped(30) }), reported({ effectiveSecs: 3600 }), NOW);
    expect(g.fraction).toBe(1);
  });

  it("ignores a paused daily cap for the fraction", () => {
    const paused: Policy[] = [
      { id: "p", scope: { kind: "device" }, budget: { tz: "UTC", dailyMinutes: 60, paused: true } },
    ];
    expect(dailyCap(child({ policies: paused }))).toBeNull();
    expect(dailyCap(child({ policies: capped(60) }))).toBe(60);
  });

  it("says Time's up, out, with an empty meter, when the budget is spent", () => {
    const g = childGlance(
      child({ policies: capped(60) }),
      reported({ locked: true, lockReason: "budget", effectiveSecs: 0 }),
      NOW,
    );
    expect(g).toMatchObject({ line: "Time's up", tone: "out", fraction: 0, online: true });
    expect(g.spoken).toBe("Sam, online, time's up for today");
  });

  it("says Stopped after a stand-down", () => {
    const g = childGlance(
      child(),
      reported({ locked: true, lockReason: "standdown", effectiveSecs: 0 }),
      NOW,
    );
    expect(g).toMatchObject({ line: "Stopped", tone: "out", fraction: null });
    expect(g.spoken).toBe("Sam, online, stopped until you allow them back on");
  });

  it("says Locked, with the opening time when it is later today", () => {
    const g = childGlance(child({ policies: afternoonsOnly() }), {}, NOW);
    expect(g).toMatchObject({ line: "Locked · 4pm", tone: "out", fraction: null });
    expect(g.spoken).toBe("Sam, online, locked. Opens at 4:00 PM");
    // Too long for the pill: say only Locked (the spoken form keeps the time).
    const half = childGlance(child({ policies: afternoonsOnly("16:30", "18:00") }), {}, NOW);
    expect(half.line).toBe("Locked");
    expect(half.spoken).toBe("Sam, online, locked. Opens at 4:30 PM");
  });

  it("says plain Locked when the next opening is not today, or unknown", () => {
    // Outside today's hours and nothing opens later today: opens tomorrow.
    const late = childGlance(child({ policies: afternoonsOnly("08:00", "09:00") }), {}, NOW);
    expect(late.line).toBe("Locked");
    expect(late.tone).toBe("out");
    expect(late.spoken).toBe("Sam, online, locked. Opens tomorrow at 8:00 AM");
    // A live schedule lock carries no opening time.
    const live = childGlance(child(), reported({ locked: true, lockReason: "schedule" }), NOW);
    expect(live.line).toBe("Locked");
    expect(live.spoken).toBe("Sam, online, locked");
  });

  it("keeps every line short enough for the pill", () => {
    const lines = [
      childGlance(child({ devices: [] }), {}, NOW).line,
      childGlance(child(), reported({ effectiveSecs: 9 * 3600 + 59 * 60 }), NOW).line,
      childGlance(child({ policies: afternoonsOnly("16:30", "18:00") }), {}, NOW).line,
      childGlance(child({ policies: afternoonsOnly("22:30", "23:00") }), {}, NOW).line,
      childGlance(child({ policies: afternoonsOnly("12:00", "23:00") }), {}, NOW).line,
    ];
    for (const l of lines) expect(l.length, l).toBeLessThanOrEqual(14);
  });

  describe("online", () => {
    it("is true when a paired device was seen within the live window", () => {
      const seen = (ago: number) =>
        childGlance(child({ devices: [device({ lastSeenAt: NOW - ago })] }), {}, NOW);
      expect(seen(LIVE_WITHIN_MS - 1000).online).toBe(true);
      expect(seen(LIVE_WITHIN_MS + 1000).online).toBe(false);
      expect(seen(LIVE_WITHIN_MS + 1000).spoken).toBe("Sam, offline, no time limit today");
    });

    it("is false for a device never seen", () => {
      const g = childGlance(child({ devices: [device({ lastSeenAt: undefined })] }), {}, NOW);
      expect(g.online).toBe(false);
    });

    it("is true if ANY device is live, and ignores one still pairing", () => {
      const quiet = device({ id: "d0", lastSeenAt: NOW - 3_600_000 });
      expect(childGlance(child({ devices: [quiet, device()] }), {}, NOW).online).toBe(true);
      const pending = device({ pairing: "pairing" });
      expect(childGlance(child({ devices: [quiet, pending] }), {}, NOW).online).toBe(false);
    });

    it("counts a home-network session as online, as the Now card does", () => {
      const stale = child({ devices: [device({ lastSeenAt: NOW - 3_600_000 })] });
      expect(childGlance(stale, {}, NOW).online).toBe(false);
      expect(childGlance(stale, {}, NOW, new Set([PUBKEY])).online).toBe(true);
    });

    it("is independent of the line: an offline child still shows their time", () => {
      const g = childGlance(
        child({ devices: [device({ lastSeenAt: NOW - 3_600_000 })] }),
        reported({ effectiveSecs: 600 }),
        NOW,
      );
      expect(g.online).toBe(false);
      expect(g.line).toBe("10m left");
      expect(g.spoken).toBe("Sam, offline, 10 minutes left today");
    });
  });
});
