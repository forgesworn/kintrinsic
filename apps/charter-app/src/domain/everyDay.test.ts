import { describe, expect, it } from "vitest";
import {
  applyToEveryDay,
  collapseSourceDay,
  isSameEveryDay,
  sharedWindows,
} from "./everyDay";
import { WEEKDAYS, type Schedule } from "./types";

function sched(weekly: Schedule["weekly"]): Schedule {
  return { tz: "Europe/London", weekly };
}

const AFTER_SCHOOL = [{ start: "16:00", end: "18:00" }];
const LIE_IN = [{ start: "10:00", end: "20:00" }];

describe("isSameEveryDay", () => {
  it("is true for a fresh, empty schedule", () => {
    expect(isSameEveryDay(sched({}))).toBe(true);
  });

  it("is true when every day carries the same window", () => {
    const weekly = Object.fromEntries(WEEKDAYS.map((d) => [d, AFTER_SCHOOL]));
    expect(isSameEveryDay(sched(weekly))).toBe(true);
  });

  // An absent day and an explicitly-empty day both mean "blocked"; treating
  // them as different would open the per-day view for no reason a parent can see.
  it("treats an absent day and an empty day as the same thing", () => {
    const weekly = Object.fromEntries(WEEKDAYS.map((d) => [d, []]));
    delete (weekly as Record<string, unknown>).sun;
    expect(isSameEveryDay(sched(weekly))).toBe(true);
  });

  it("is false as soon as one day differs", () => {
    const weekly = Object.fromEntries(WEEKDAYS.map((d) => [d, AFTER_SCHOOL]));
    weekly.sat = LIE_IN;
    expect(isSameEveryDay(sched(weekly))).toBe(false);
  });

  it("is false when the same times are split into different windows", () => {
    const weekly = Object.fromEntries(WEEKDAYS.map((d) => [d, AFTER_SCHOOL]));
    weekly.fri = [
      { start: "16:00", end: "17:00" },
      { start: "17:00", end: "18:00" },
    ];
    expect(isSameEveryDay(sched(weekly))).toBe(false);
  });
});

describe("collapseSourceDay", () => {
  /// Collapsing a weekend-only charter onto a blocked Monday would silently
  /// turn the entire week off — the opposite of what a parent reaching for
  /// "same every day" is asking for.
  it("takes the first day that actually allows something", () => {
    expect(collapseSourceDay(sched({ sat: LIE_IN }))).toBe("sat");
  });

  it("prefers the earliest allowed day when several qualify", () => {
    expect(collapseSourceDay(sched({ wed: AFTER_SCHOOL, sat: LIE_IN }))).toBe("wed");
  });

  it("falls back to Monday when nothing is allowed at all", () => {
    expect(collapseSourceDay(sched({}))).toBe("mon");
  });
});

describe("applyToEveryDay", () => {
  it("writes the given windows to all seven days", () => {
    const out = applyToEveryDay(sched({ sat: LIE_IN }), AFTER_SCHOOL);
    for (const d of WEEKDAYS) expect(out.weekly[d]).toEqual(AFTER_SCHOOL);
    expect(isSameEveryDay(out)).toBe(true);
  });

  it("keeps the rest of the schedule untouched", () => {
    const before = { ...sched({}), tz: "Europe/London", paused: true };
    const out = applyToEveryDay(before, AFTER_SCHOOL);
    expect(out.tz).toBe("Europe/London");
    expect(out.paused).toBe(true);
  });

  // Shared references would make a later per-day edit silently change its
  // siblings — the bug this whole feature could most easily introduce.
  it("gives every day its own copy, not a shared reference", () => {
    const out = applyToEveryDay(sched({}), AFTER_SCHOOL);
    out.weekly.mon![0].start = "09:00";
    expect(out.weekly.tue![0].start).toBe("16:00");
    // …and never aliases the caller's input either.
    expect(AFTER_SCHOOL[0].start).toBe("16:00");
  });

  it("can block the whole week", () => {
    const out = applyToEveryDay(sched({ mon: AFTER_SCHOOL }), []);
    for (const d of WEEKDAYS) expect(out.weekly[d]).toEqual([]);
  });
});

describe("sharedWindows", () => {
  it("reads from the source day", () => {
    expect(sharedWindows(sched({ sat: LIE_IN }))).toEqual(LIE_IN);
  });

  it("is empty when nothing is allowed", () => {
    expect(sharedWindows(sched({}))).toEqual([]);
  });

  it("hands back a copy the caller can edit freely", () => {
    const s = sched({ mon: AFTER_SCHOOL });
    sharedWindows(s)[0].start = "09:00";
    expect(s.weekly.mon![0].start).toBe("16:00");
  });
});
