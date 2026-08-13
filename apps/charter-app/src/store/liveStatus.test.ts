import { describe, it, expect } from "vitest";
import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { freshestStatusFor, liveStatusFor } from "./liveStatus";

const child: Child = {
  id: "c",
  name: "Sam",
  color: "#fff",
  dependantPubkey: null,
  devices: [],
  policies: [],
};

function feed(over: Partial<DeviceStatus>): DeviceStatus {
  return {
    v: 1,
    subject: "cd".repeat(32),
    machine: "ab".repeat(32),
    ts: 1000,
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

// feed.ts = 1000s; a `now` of 1000*1000 ms → now/1000 == 1000 == ts → fresh.
const NOW_FRESH = 1000 * 1000;

describe("liveStatusFor", () => {
  it("prefers a fresh feed and shows its time-left", () => {
    const r = liveStatusFor(child, feed({ effectiveSecs: 1800 }), NOW_FRESH);
    expect(r.live).toBe(true);
    expect(r.allowedNow).toBe(true);
    expect(r.minutesLeftToday).toBe(30);
    expect(r.locked).toBe(false);
    expect(r.asOf).toBe(1000);
  });

  // The contract gives StandDown its own lockReason PURELY so people are told
  // which wall was met — and the guardian is a person too. Mapping it to
  // "schedule" made Home say "Outside of allowed hours" two lines above
  // FinishNow saying "Finished for today — waiting on you".
  it("passes a stand-down lock through as its own reason", () => {
    const r = liveStatusFor(
      child,
      feed({ locked: true, lockReason: "standdown", effectiveSecs: 0 }),
      NOW_FRESH,
    );
    expect(r.locked).toBe(true);
    expect(r.reason).toBe("standdown");
  });

  it("reflects a locked feed with its reason", () => {
    const r = liveStatusFor(child, feed({ locked: true, lockReason: "budget", effectiveSecs: 0 }), NOW_FRESH);
    expect(r.live).toBe(true);
    expect(r.allowedNow).toBe(false);
    expect(r.locked).toBe(true);
    expect(r.reason).toBe("budget");
    expect(r.minutesLeftToday).toBe(0);
  });

  // The card counts the last stretch down in seconds, so it needs the real
  // remainder — whole minutes lose up to 59 seconds of it.
  it("carries the exact seconds a live feed reported", () => {
    const r = liveStatusFor(child, feed({ effectiveSecs: 150 }), NOW_FRESH);
    expect(r.secondsLeftToday).toBe(150);
    expect(r.minutesLeftToday).toBe(2);
  });

  // The local guess has no usage input, so it cannot honestly claim seconds —
  // a made-up countdown is worse than none.
  it("claims no seconds when it is only guessing", () => {
    const r = liveStatusFor(child, undefined, NOW_FRESH);
    expect(r.live).toBe(false);
    expect(r.secondsLeftToday).toBeUndefined();
  });

  // An unbounded day is flattened to 0 on the wire (STATUS carries unsigned
  // seconds, and the enforcer's -1 saturates). Rendered literally that reads
  // as "about to lock" — the opposite of the truth — so an unbounded day has
  // to be recognised, not taken at face value.
  it("reads an unconstrained ward as having no limit, not zero left", () => {
    const r = liveStatusFor(
      child,
      feed({ effectiveSecs: 0, locked: false, source: "unconstrained" }),
      NOW_FRESH,
    );
    expect(r.minutesLeftToday).toBeNull();
    expect(r.secondsLeftToday).toBeUndefined();
    expect(r.allowedNow).toBe(true);
  });

  // A charter whose schedule is PAUSED ("allow anytime") with no daily cap is
  // equally unbounded, but still reports source "guardian". The app signed
  // that charter, so it can tell — and must not cry "<1m left".
  it("reads a paused, uncapped charter as having no limit", () => {
    const paused: Child = {
      ...child,
      policies: [
        {
          id: "p1",
          scope: { kind: "device" },
          schedule: { tz: "Europe/London", weekly: {}, paused: true },
        },
      ],
    };
    const r = liveStatusFor(
      paused,
      feed({ effectiveSecs: 0, locked: false, source: "guardian" }),
      NOW_FRESH,
    );
    expect(r.minutesLeftToday).toBeNull();
  });

  // But a ward who has genuinely just run out, in the tick before the lock
  // lands, must NOT be reported as unlimited.
  it("still reports a real zero for a capped ward about to lock", () => {
    const capped: Child = {
      ...child,
      policies: [
        {
          id: "p1",
          scope: { kind: "device" },
          budget: { tz: "Europe/London", dailyMinutes: 60 },
        },
      ],
    };
    const r = liveStatusFor(
      capped,
      feed({ effectiveSecs: 0, locked: false, source: "guardian" }),
      NOW_FRESH,
    );
    expect(r.minutesLeftToday).toBe(0);
  });

  it("falls back to the local guess when the feed is stale", () => {
    const stale = 10_000 * 1000; // feed.ts=1000s → 9000s old > 180
    const r = liveStatusFor(child, feed({}), stale);
    expect(r.live).toBe(false);
  });

  it("falls back when there is no feed at all", () => {
    const r = liveStatusFor(child, undefined, NOW_FRESH);
    expect(r.live).toBe(false);
    expect(r.allowedNow).toBe(true); // no device policy → allowed
  });
});

describe("freshestStatusFor", () => {
  const PHONE = "11".repeat(32);
  const LAPTOP = "22".repeat(32);
  const twoDevices: Child = {
    ...child,
    devices: [
      { id: "d1", label: "phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
      { id: "d2", label: "laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
    ],
  };
  const NOW = 1_000_000 * 1000; // epoch ms

  /// The bug decented hit on 2026-07-27: he called a stand-down, watched the ward
  /// get the warning and the lock, and the card said "Allowed now — live"
  /// because a second device had simply reported a moment later.
  it("lets a LOCKED device speak for the ward even when another beat later", () => {
    const now = NOW / 1000;
    const got = freshestStatusFor(
      twoDevices,
      {
        [PHONE]: feed({ machine: PHONE, ts: now - 30, locked: true }),
        [LAPTOP]: feed({ machine: LAPTOP, ts: now - 1, locked: false }),
      },
      NOW,
    );
    expect(got?.machine).toBe(PHONE);
    expect(liveStatusFor(twoDevices, got, NOW).allowedNow).toBe(false);
  });

  it("uses the freshest when nothing is locked", () => {
    const now = NOW / 1000;
    const got = freshestStatusFor(
      twoDevices,
      {
        [PHONE]: feed({ machine: PHONE, ts: now - 30, locked: false }),
        [LAPTOP]: feed({ machine: LAPTOP, ts: now - 1, locked: false }),
      },
      NOW,
    );
    expect(got?.machine).toBe(LAPTOP);
  });

  // A lock nobody has confirmed for minutes is not evidence of anything; let
  // liveStatusFor judge staleness exactly as it did before.
  it("does not let a STALE lock outrank a fresh unlocked report", () => {
    const now = NOW / 1000;
    const got = freshestStatusFor(
      twoDevices,
      {
        [PHONE]: feed({ machine: PHONE, ts: now - 10_000, locked: true }),
        [LAPTOP]: feed({ machine: LAPTOP, ts: now - 1, locked: false }),
      },
      NOW,
    );
    expect(got?.machine).toBe(LAPTOP);
  });

  it("still returns something when every report is stale", () => {
    const now = NOW / 1000;
    const got = freshestStatusFor(
      twoDevices,
      { [PHONE]: feed({ machine: PHONE, ts: now - 10_000, locked: true }) },
      NOW,
    );
    expect(got?.machine).toBe(PHONE);
    // …and liveStatusFor still treats it as not-live.
    expect(liveStatusFor(twoDevices, got, NOW).live).toBe(false);
  });

  it("has nothing to say for a ward with no paired devices", () => {
    expect(freshestStatusFor(child, {}, NOW)).toBeUndefined();
  });
});
