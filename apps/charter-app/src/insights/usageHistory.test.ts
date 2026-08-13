import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  decodeMinutes,
  emptyMinutes,
  encodeMinutes,
  minuteCount,
  setMinute,
} from "./minuteSet";
import {
  emptyUsageHistory,
  emptyWeekMessage,
  humanDuration,
  outOfHoursLine,
  pruneHistory,
  recordUsage,
  weekSummary,
  weeklyView,
} from "./usageHistory";

// A fixed "now": 2026-07-24 (Fri) 12:00 local.
const NOW = new Date(2026, 6, 24, 12, 0, 0).getTime();
const PHONE = "p".repeat(64);
const LAPTOP = "l".repeat(64);

describe("recordUsage", () => {
  it("keeps the MAX seen per device per day (monotonic), no-op on a lower value", () => {
    let h = recordUsage(emptyUsageHistory, PHONE, "2026-07-24", 600);
    expect(h.byMachine[PHONE]["2026-07-24"]).toBe(600);
    const same = recordUsage(h, PHONE, "2026-07-24", 400); // lower → no change
    expect(same).toBe(h); // same reference
    h = recordUsage(h, PHONE, "2026-07-24", 900);
    expect(h.byMachine[PHONE]["2026-07-24"]).toBe(900);
  });

  it("rejects junk without throwing", () => {
    expect(recordUsage(emptyUsageHistory, PHONE, "not-a-day", 60)).toBe(emptyUsageHistory);
    expect(recordUsage(emptyUsageHistory, PHONE, "2026-07-24", -5)).toBe(emptyUsageHistory);
    expect(recordUsage(emptyUsageHistory, "", "2026-07-24", 60)).toBe(emptyUsageHistory);
  });

  it("does not mutate the input", () => {
    const h0 = emptyUsageHistory;
    recordUsage(h0, PHONE, "2026-07-24", 600);
    expect(h0.byMachine[PHONE]).toBeUndefined();
  });
});

describe("weeklyView", () => {
  const h = [
    [PHONE, "2026-07-24", 3 * 3600], // today, phone 3h
    [LAPTOP, "2026-07-24", 30 * 60], // today, laptop 30m  → total 3h30 (over 2h)
    [PHONE, "2026-07-22", 90 * 60], // Wed, 1h30 (under)
  ].reduce((acc, [m, d, s]) => recordUsage(acc, m as string, d as string, s as number), emptyUsageHistory);

  it("returns the 7 days ending today, oldest first", () => {
    const w = weeklyView(h, [PHONE, LAPTOP], 120, NOW);
    expect(w).toHaveLength(7);
    expect(w[6].dayKey).toBe("2026-07-24");
    expect(w[6].label).toBe("Fri");
    expect(w[0].dayKey).toBe("2026-07-18");
  });

  it("sums across the child's devices and computes the overdraft against the allowance", () => {
    const w = weeklyView(h, [PHONE, LAPTOP], 120, NOW); // allowance 2h
    const today = w[6];
    expect(today.totalSecs).toBe(3 * 3600 + 30 * 60); // 3h30
    expect(today.perDevice).toEqual([
      { machine: PHONE, secs: 3 * 3600 },
      { machine: LAPTOP, secs: 30 * 60 },
    ]);
    expect(today.overdraftSecs).toBe(90 * 60); // 3h30 - 2h = 1h30 over
    const wed = w.find((d) => d.dayKey === "2026-07-22")!;
    expect(wed.totalSecs).toBe(90 * 60);
    expect(wed.overdraftSecs).toBe(0); // under
  });

  it("empty days are zero and never negative overdraft", () => {
    const w = weeklyView(h, [PHONE, LAPTOP], 120, NOW);
    const empty = w.find((d) => d.dayKey === "2026-07-20")!;
    expect(empty.totalSecs).toBe(0);
    expect(empty.overdraftSecs).toBe(0);
  });

  it("no allowance set → no overdraft ever", () => {
    const w = weeklyView(h, [PHONE, LAPTOP], null, NOW);
    expect(w[6].overdraftSecs).toBe(0);
    expect(w[6].allowanceSecs).toBe(0);
  });
});

describe("weekSummary & humanDuration", () => {
  it("humanizes durations", () => {
    expect(humanDuration(45 * 60)).toBe("45m");
    expect(humanDuration(60 * 60)).toBe("1h");
    expect(humanDuration(90 * 60)).toBe("1h 30m");
  });

  it("summary is calm and reflects total + days over", () => {
    const days = weeklyView(
      recordUsage(
        recordUsage(emptyUsageHistory, PHONE, "2026-07-24", 3 * 3600),
        LAPTOP,
        "2026-07-24",
        30 * 60,
      ),
      [PHONE, LAPTOP],
      120,
      NOW,
    );
    expect(weekSummary(days)).toBe("This week: 3h 30m · 1 day over");
  });

  it("empty week has a gentle message, not a zero", () => {
    expect(weekSummary(weeklyView(emptyUsageHistory, [PHONE], 120, NOW))).toBe(
      "No screen time recorded yet this week.",
    );
  });
});

// Frozen cross-language vectors (review fix I2, 2026-08-04): the SAME file
// android/app/src/test/kotlin/org/forgesworn/charter/ui/GroupMirrorTest.kt
// asserts, so a one-sided edit to either language's `outOfHoursLine` shows up
// as a failing test instead of a silently divergent sentence between the
// guardian's PWA and the ward's own device.
const OOH_VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/usage/out_of_hours_line_vectors.json",
);
interface OutOfHoursVectorDoc {
  vectors: { name: string; nights: number; secs: number; expected: string | null }[];
}
const oohDoc = JSON.parse(readFileSync(OOH_VECTORS_PATH, "utf8")) as OutOfHoursVectorDoc;

describe("outOfHoursLine (frozen cross-language vectors)", () => {
  it.each(oohDoc.vectors)("$name", ({ nights, secs, expected }) => {
    expect(outOfHoursLine(nights, secs)).toBe(expected);
  });

  // A family that never sets the clause must see no line at all, not a zero
  // — re-asserted directly (not just via the vector table above) since it's
  // the load-bearing guarantee `WeeklyPicture`'s empty-state coherence relies
  // on (see `emptyWeekMessage` below).
  it("says nothing when there is nothing to say", () => {
    expect(outOfHoursLine(0, 0)).toBeNull();
  });
});

describe("emptyWeekMessage", () => {
  it("keeps the original gentle message when there was no out-of-hours use either", () => {
    expect(emptyWeekMessage(false)).toBe(
      "No screen time recorded yet this week — it fills in as the devices are used.",
    );
  });

  // Review fix C1 (2026-08-04): a ward whose whole week's use was an
  // always-available app during locked hours has zero COUNTED screen time
  // (out-of-hours seconds are deliberately never added to the budget total)
  // while a real out-of-hours line is about to render right below this
  // message. The blanket "No screen time recorded" claim would be false —
  // there WAS recorded activity, it just isn't counted screen time — so the
  // message must narrow itself rather than contradict the line beneath it.
  it("narrows to 'no COUNTED screen time' when out-of-hours use is real, never the blanket claim", () => {
    const msg = emptyWeekMessage(true);
    expect(msg).toBe("No counted screen time this week.");
    expect(msg).not.toContain("No screen time recorded yet this week");
  });
});

describe("pruneHistory", () => {
  it("drops days older than the keep window", () => {
    const h = recordUsage(emptyUsageHistory, PHONE, "2026-01-01", 60);
    const pruned = pruneHistory(h, NOW, 60);
    expect(pruned.byMachine[PHONE]["2026-01-01"]).toBeUndefined();
  });

  it("prunes minute journals with the same window", () => {
    const bm = encodeMinutes(bits([1, 2, 3]));
    const h = recordUsage(emptyUsageHistory, PHONE, "2026-01-01", 60, bm);
    const pruned = pruneHistory(h, NOW, 60);
    expect(pruned.minutesByMachine?.[PHONE]?.["2026-01-01"]).toBeUndefined();
  });
});

// ---- B3: minute journals + the union rule --------------------------------

function bits(minutes: number[]): Uint8Array {
  const b = emptyMinutes();
  for (const m of minutes) setMinute(b, m);
  return b;
}
const range = (a: number, n: number) => Array.from({ length: n }, (_, i) => a + i);

describe("minute journals", () => {
  it("stores a device's journal and merges across polls by union", () => {
    let h = recordUsage(emptyUsageHistory, PHONE, "2026-07-24", 600, encodeMinutes(bits([10, 11])));
    h = recordUsage(h, PHONE, "2026-07-24", 600, encodeMinutes(bits([11, 12])));
    const merged = decodeMinutes(h.minutesByMachine![PHONE]["2026-07-24"])!;
    expect(minuteCount(merged)).toBe(3); // {10,11,12}
  });

  it("same journal re-polled is a no-op (same reference)", () => {
    const bm = encodeMinutes(bits([10, 11]));
    const h = recordUsage(emptyUsageHistory, PHONE, "2026-07-24", 600, bm);
    expect(recordUsage(h, PHONE, "2026-07-24", 600, bm)).toBe(h);
  });

  it("union day total counts simultaneous use once, with the overlap exposed", () => {
    // Phone: 40 minutes (0..39). Laptop: 30 minutes (20..49). Union = 50m.
    let h = recordUsage(
      emptyUsageHistory,
      PHONE,
      "2026-07-24",
      40 * 60,
      encodeMinutes(bits(range(0, 40))),
    );
    h = recordUsage(h, LAPTOP, "2026-07-24", 30 * 60, encodeMinutes(bits(range(20, 30))));
    const today = weeklyView(h, [PHONE, LAPTOP], 120, NOW)[6];
    expect(today.totalSecs).toBe(50 * 60); // NOT 70m
    expect(today.overlapSecs).toBe(20 * 60); // both-at-once
    expect(today.overdraftSecs).toBe(0); // under the 2h allowance
  });

  it("falls back to the scalar sum when any using device lacks a journal", () => {
    let h = recordUsage(
      emptyUsageHistory,
      PHONE,
      "2026-07-24",
      40 * 60,
      encodeMinutes(bits(range(0, 40))),
    );
    h = recordUsage(h, LAPTOP, "2026-07-24", 30 * 60); // no journal
    const today = weeklyView(h, [PHONE, LAPTOP], 120, NOW)[6];
    expect(today.totalSecs).toBe(70 * 60); // honest scalar sum
    expect(today.overlapSecs).toBe(0);
  });
});
