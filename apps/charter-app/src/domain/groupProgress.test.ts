import { describe, expect, it } from "vitest";
import {
  groupExtrasToday,
  groupProgressBuckets,
  groupProgressLine,
  groupProgressRows,
  type GroupProgressRow,
} from "./groupProgress";
import type { ActivityEvent, AppBucketRule, BucketsPolicy } from "./types";
import type { StatusGroup } from "../wire/status";

const BUCKETS: AppBucketRule[] = [
  { id: "play", label: "Play", apps: [], dailyMinutes: 60, weeklyMinutes: 300 },
  { id: "creative", label: "Creative", apps: [], dailyMinutes: 30 },
  { id: "social", label: "Social", apps: [], weeklyMinutes: 120 },
];

describe("groupProgressRows", () => {
  it("joins STATUS groups to the saved bucket's label + caps", () => {
    const groups: StatusGroup[] = [{ id: "play", daySecs: 2_700, weekSecs: 7_200 }];
    const rows = groupProgressRows(groups, BUCKETS);
    expect(rows).toEqual([
      { id: "play", label: "Play", daySecs: 2_700, weekSecs: 7_200, dailyMinutes: 60, weeklyMinutes: 300 },
    ]);
  });

  it("absent STATUS groups (a ward whose device predates named times) yields nothing — never zeros", () => {
    expect(groupProgressRows(undefined, BUCKETS)).toEqual([]);
    expect(groupProgressRows([], BUCKETS)).toEqual([]);
  });

  it("a group STATUS reports but the policy no longer names renders by its raw id, uncapped", () => {
    const groups: StatusGroup[] = [{ id: "ghost", daySecs: 600, weekSecs: 600 }];
    const rows = groupProgressRows(groups, BUCKETS);
    expect(rows).toEqual([
      { id: "ghost", label: "ghost", daySecs: 600, weekSecs: 600, dailyMinutes: undefined, weeklyMinutes: undefined },
    ]);
  });

  it("works with an absent policy entirely — every STATUS group renders raw", () => {
    const groups: StatusGroup[] = [{ id: "play", daySecs: 60, weekSecs: 60 }];
    expect(groupProgressRows(groups, undefined)).toEqual([
      { id: "play", label: "play", daySecs: 60, weekSecs: 60, dailyMinutes: undefined, weeklyMinutes: undefined },
    ]);
  });

  it("threads today's extras onto the matching row only (M-2)", () => {
    const groups: StatusGroup[] = [
      { id: "play", daySecs: 2_700, weekSecs: 7_200 },
      { id: "creative", daySecs: 600, weekSecs: 600 },
    ];
    const rows = groupProgressRows(groups, BUCKETS, { play: 15 });
    expect(rows.find((r) => r.id === "play")?.extraMinutesToday).toBe(15);
    expect(rows.find((r) => r.id === "creative")?.extraMinutesToday).toBeUndefined();
  });

  it("omitting the extras map leaves every row's extraMinutesToday undefined", () => {
    const groups: StatusGroup[] = [{ id: "play", daySecs: 2_700, weekSecs: 7_200 }];
    expect(groupProgressRows(groups, BUCKETS)[0].extraMinutesToday).toBeUndefined();
  });
});

describe("groupProgressBuckets (F2: a paused Counted-times set draws no wall)", () => {
  const policy = (over: Partial<BucketsPolicy> = {}): BucketsPolicy => ({
    enabled: true,
    tz: "UTC",
    buckets: BUCKETS,
    ...over,
  });

  it("passes the caps through when Counted times is enabled", () => {
    expect(groupProgressBuckets(policy())).toBe(BUCKETS);
  });

  it("drops the caps (undefined) when Counted times is paused — no wall for a spent-time set that isn't enforced", () => {
    expect(groupProgressBuckets(policy({ enabled: false }))).toBeUndefined();
  });

  it("is undefined for an absent policy, same as an untouched screen", () => {
    expect(groupProgressBuckets(undefined)).toBeUndefined();
  });

  // The whole point: feeding a paused policy's buckets through
  // `groupProgressRows` must render the CAP-LESS, honest raw-meter row — the
  // group still shows what was spent, just no "of X" wall implying an
  // enforcement that stopped.
  it("end-to-end: a paused set's STATUS group renders raw meters, not a wall", () => {
    const groups: StatusGroup[] = [{ id: "play", daySecs: 2_700, weekSecs: 7_200 }];
    const rows = groupProgressRows(groups, groupProgressBuckets(policy({ enabled: false })));
    expect(rows).toEqual([
      { id: "play", label: "play", daySecs: 2_700, weekSecs: 7_200, dailyMinutes: undefined, weeklyMinutes: undefined },
    ]);
    expect(groupProgressLine(rows[0])).not.toContain(" of ");
  });
});

describe("groupProgressLine", () => {
  const row = (over: Partial<GroupProgressRow> = {}): GroupProgressRow => ({
    id: "play",
    label: "Play",
    daySecs: 0,
    weekSecs: 0,
    ...over,
  });

  it("shows both axes with 'of X' only when both are capped", () => {
    const line = groupProgressLine(
      row({ daySecs: 45 * 60, dailyMinutes: 60, weekSecs: 130 * 60, weeklyMinutes: 300 }),
    );
    expect(line).toBe("Play — 45m of 1h today · 2h 10m of 5h this week");
  });

  it("shows ONLY the daily axis when only a daily cap is set", () => {
    const line = groupProgressLine(row({ daySecs: 20 * 60, dailyMinutes: 30, weekSecs: 999 * 60 }));
    expect(line).toBe("Play — 20m of 30m today");
  });

  it("shows ONLY the weekly axis when only a weekly cap is set", () => {
    const line = groupProgressLine(row({ weekSecs: 60 * 60, weeklyMinutes: 120, daySecs: 999 * 60 }));
    expect(line).toBe("Play — 1h of 2h this week");
  });

  it("an uncapped (unknown) group shows raw meters on both axes, never 'of X'", () => {
    const line = groupProgressLine(row({ daySecs: 5 * 60, weekSecs: 65 * 60 }));
    expect(line).toBe("Play — 5m today · 1h 5m this week");
    expect(line).not.toContain(" of ");
  });

  it("formats whole hours without a trailing 0m", () => {
    const line = groupProgressLine(row({ daySecs: 60 * 60, dailyMinutes: 60 }));
    expect(line).toBe("Play — 1h of 1h today");
  });

  // M-2 (hardware round, 2026-08-03): a grant the guardian personally
  // approved must not read back as a breach of the BASE cap.
  describe("extraMinutesToday (M-2)", () => {
    it("adds the extra onto BOTH capped axes and names it", () => {
      const line = groupProgressLine(
        row({
          daySecs: 30 * 60,
          dailyMinutes: 15,
          weekSecs: 30 * 60,
          weeklyMinutes: 30,
          extraMinutesToday: 15,
        }),
      );
      expect(line).toBe(
        "Play — 30m of 30m today · 30m of 45m this week (includes 15m extra you gave)",
      );
      // The exact regression: never reads as "spent > cap".
      expect(line).not.toContain("30m of 15m");
    });

    it("adds nothing and stays silent when there's no extra", () => {
      const line = groupProgressLine(row({ daySecs: 5 * 60, dailyMinutes: 15 }));
      expect(line).not.toContain("extra you gave");
    });

    it("an uncapped row still names the extra even though there's no cap to bump", () => {
      const line = groupProgressLine(row({ daySecs: 20 * 60, extraMinutesToday: 10 }));
      expect(line).toBe("Play — 20m today · 0m this week (includes 10m extra you gave)");
    });
  });
});

describe("groupExtrasToday (M-2)", () => {
  const CHILD = "child1";
  const TODAY_START_MS = 1_700_000_000_000;

  function ev(over: Partial<ActivityEvent> = {}): ActivityEvent {
    return {
      id: `act-${Math.random()}`,
      childId: CHILD,
      ts: TODAY_START_MS + 60_000,
      outcome: "enacted",
      summary: "You approved: More Play time (+15 min)",
      bucketId: "play",
      minutesGranted: 15,
      ...over,
    };
  }

  it("sums same-group extends and gifts together", () => {
    const activity = [
      ev({ minutesGranted: 15 }),
      ev({ outcome: "rule-changed", summary: "You gave 10 more minutes to Play", minutesGranted: 10 }),
    ];
    expect(groupExtrasToday(activity, CHILD, TODAY_START_MS)).toEqual({ play: 25 });
  });

  it("keeps different groups separate", () => {
    const activity = [ev({ bucketId: "play", minutesGranted: 15 }), ev({ bucketId: "social", minutesGranted: 20 })];
    expect(groupExtrasToday(activity, CHILD, TODAY_START_MS)).toEqual({ play: 15, social: 20 });
  });

  it("ignores another child's activity", () => {
    const activity = [ev({ childId: "child2" })];
    expect(groupExtrasToday(activity, CHILD, TODAY_START_MS)).toEqual({});
  });

  it("ignores activity from before today's start — no stale day rolled forward", () => {
    const activity = [ev({ ts: TODAY_START_MS - 1 })];
    expect(groupExtrasToday(activity, CHILD, TODAY_START_MS)).toEqual({});
  });

  it("ignores whole-device gifts/activity (no bucketId) and every non-grant event", () => {
    const activity = [
      ev({ bucketId: undefined, minutesGranted: undefined, summary: "You gave 10 more minutes" }),
      ev({ bucketId: undefined, minutesGranted: undefined, outcome: "rule-changed", summary: "You changed their schedule" }),
    ];
    expect(groupExtrasToday(activity, CHILD, TODAY_START_MS)).toEqual({});
  });

  it("empty activity yields an empty map", () => {
    expect(groupExtrasToday([], CHILD, TODAY_START_MS)).toEqual({});
  });
});
