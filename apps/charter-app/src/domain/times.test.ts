import { describe, expect, it } from "vitest";
import type { Device, NamedTime, Policy, Schedule } from "./types";
import type { DeviceStatus } from "../wire/status";
import { SPLITTABLE_CONTROLS, effectivePolicyForDevice } from "./effectivePolicy";
import {
  activeTimeAt,
  addNamedTime,
  canUseTimes,
  customTimeTemplate,
  defaultExtendTime,
  findTaggedOverlaps,
  learningTimeTemplate,
  namedTimeProblems,
  nextWindow,
  removeNamedTime,
  renameNamedTime,
  setTimeWindows,
  timeSlug,
  unknownTimeTags,
  updateNamedTime,
} from "./times";
import { wardenSupport } from "./wardenSupport";
import { policyToClauses } from "../wire/clause";

/** Unix seconds for a UTC wall time. Mon 2026-11-02 is a Monday. */
const utc = (date: string, hhmm: string): number => Date.parse(`${date}T${hhmm}:00Z`) / 1000;
const MON = "2026-11-02";
const TUE = "2026-11-03";

const learning: NamedTime = {
  id: "learning",
  label: "Learning time",
  posture: "onlyThese",
  apps: ["org.example.maths"],
};
const quiet: NamedTime = { id: "quiet", label: "Quiet time", posture: "everythingExcept", blockApps: ["a.b"] };
const times = [learning, quiet];

/** Monday: Learning 09:00-12:00 (tagged), Free 16:00-19:00. */
const sched: Schedule = {
  tz: "UTC",
  weekly: {
    mon: [
      { start: "09:00", end: "12:00", time: "learning" },
      { start: "16:00", end: "19:00" },
    ],
  },
};

describe("activeTimeAt — the device rule, guardian-side", () => {
  it("is the tagged time inside a tagged window, Free inside an untagged one", () => {
    expect(activeTimeAt(sched, times, utc(MON, "10:00"))).toEqual({ state: "time", time: learning });
    expect(activeTimeAt(sched, times, utc(MON, "17:00"))).toEqual({ state: "free" });
  });

  it("is locked outside every window; the end instant is outside", () => {
    expect(activeTimeAt(sched, times, utc(MON, "13:00"))).toEqual({ state: "locked", reason: "outside" });
    expect(activeTimeAt(sched, times, utc(MON, "12:00"))).toEqual({ state: "locked", reason: "outside" });
    expect(activeTimeAt(sched, times, utc(MON, "09:00")).state).toBe("time"); // start is inside
    expect(activeTimeAt(sched, times, utc(TUE, "10:00"))).toEqual({ state: "locked", reason: "outside" });
  });

  it("a tagged window beats an untagged one where they overlap (carving)", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "16:00", end: "19:00" },
          { start: "17:00", end: "18:00", time: "learning" },
        ],
      },
    };
    expect(activeTimeAt(s, times, utc(MON, "16:30")).state).toBe("free");
    expect(activeTimeAt(s, times, utc(MON, "17:30"))).toEqual({ state: "time", time: learning });
    expect(activeTimeAt(s, times, utc(MON, "18:00")).state).toBe("free");
  });

  it("two tagged windows (a device may still receive one): earlier start, then array order", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "10:00", end: "12:00", time: "quiet" },
          { start: "09:00", end: "12:00", time: "learning" },
        ],
      },
    };
    expect(activeTimeAt(s, times, utc(MON, "10:30"))).toEqual({ state: "time", time: learning });
    const tie: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "12:00", time: "quiet" },
          { start: "09:00", end: "12:00", time: "learning" },
        ],
      },
    };
    expect(activeTimeAt(tie, times, utc(MON, "10:00"))).toEqual({ state: "time", time: quiet });
  });

  it("an unknown tag LOCKS the span — it is never read as Free", () => {
    expect(activeTimeAt(sched, [quiet], utc(MON, "10:00"))).toEqual({
      state: "locked",
      reason: "unknown-time",
      tag: "learning",
    });
    expect(activeTimeAt(sched, undefined, utc(MON, "10:00")).state).toBe("locked");
    // ... even under an untagged window that would otherwise run Free.
    const over: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "12:00" },
          { start: "10:00", end: "11:00", time: "gone" },
        ],
      },
    };
    expect(activeTimeAt(over, times, utc(MON, "10:30")).state).toBe("locked");
    expect(activeTimeAt(over, times, utc(MON, "09:30")).state).toBe("free");
  });

  it("a date override replaces the weekly pattern, and an empty one is a closed day", () => {
    const withOverride: Schedule = {
      ...sched,
      overrides: { [MON]: [{ start: "08:00", end: "09:00", time: "quiet" }] },
    };
    expect(activeTimeAt(withOverride, times, utc(MON, "10:00")).state).toBe("locked");
    expect(activeTimeAt(withOverride, times, utc(MON, "08:30"))).toEqual({ state: "time", time: quiet });
    expect(activeTimeAt({ ...sched, overrides: { [MON]: [] } }, times, utc(MON, "17:00")).state).toBe("locked");
  });

  it("an absent or lifted schedule is Free", () => {
    expect(activeTimeAt(undefined, times, utc(MON, "03:00"))).toEqual({ state: "free" });
    expect(activeTimeAt({ ...sched, paused: true }, times, utc(MON, "03:00"))).toEqual({ state: "free" });
  });

  it("reads the day in the SCHEDULE's tz, not UTC", () => {
    const tokyo: Schedule = { tz: "Asia/Tokyo", weekly: { tue: [{ start: "08:00", end: "09:00", time: "learning" }] } };
    // Mon 23:30 UTC is Tue 08:30 in Tokyo.
    expect(activeTimeAt(tokyo, times, utc(MON, "23:30"))).toEqual({ state: "time", time: learning });
  });
});

describe("defaultExtendTime", () => {
  it("inside a window: the time already running (Free ⇒ undefined)", () => {
    expect(defaultExtendTime(sched, times, utc(MON, "10:00"))).toBe("learning");
    expect(defaultExtendTime(sched, times, utc(MON, "17:00"))).toBeUndefined();
  });

  it("locked: the time whose window most recently ended today", () => {
    expect(defaultExtendTime(sched, times, utc(MON, "13:00"))).toBe("learning");
    // Free ended most recently → Free, even though Learning is a named time.
    expect(defaultExtendTime(sched, times, utc(MON, "20:00"))).toBeUndefined();
  });

  it("picks the LATEST end when several have ended", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "08:00", end: "09:00", time: "quiet" },
          { start: "09:00", end: "10:00", time: "learning" },
          { start: "18:00", end: "19:00" },
        ],
      },
    };
    expect(defaultExtendTime(s, times, utc(MON, "11:00"))).toBe("learning");
  });

  it("nothing ended yet: the next one up today", () => {
    expect(defaultExtendTime(sched, times, utc(MON, "07:00"))).toBe("learning");
    const lateOnly: Schedule = { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } };
    expect(defaultExtendTime(lateOnly, times, utc(MON, "07:00"))).toBeUndefined();
  });

  it("no windows today (or no schedule): Free", () => {
    expect(defaultExtendTime(sched, times, utc(TUE, "10:00"))).toBeUndefined();
    expect(defaultExtendTime(undefined, times, utc(MON, "10:00"))).toBeUndefined();
  });

  it("skips a window whose time is not defined", () => {
    expect(defaultExtendTime(sched, [quiet], utc(MON, "13:00"))).toBeUndefined();
  });
});

describe("nextWindow", () => {
  it("is the next window still to open today, with the time that runs", () => {
    expect(nextWindow(sched, utc(MON, "07:00"))).toEqual({
      time: "learning",
      startsAt: utc(MON, "09:00"),
      hhmm: "09:00",
      day: "today",
    });
    expect(nextWindow(sched, utc(MON, "13:00"))).toEqual({
      startsAt: utc(MON, "16:00"),
      hhmm: "16:00",
      day: "today",
    });
  });

  it("else tomorrow's first, else null", () => {
    const s: Schedule = { tz: "UTC", weekly: { ...sched.weekly, tue: [{ start: "15:00", end: "16:00", time: "quiet" }] } };
    expect(nextWindow(s, utc(MON, "20:00"))).toEqual({
      time: "quiet",
      startsAt: utc(TUE, "15:00"),
      hhmm: "15:00",
      day: "tomorrow",
    });
    expect(nextWindow(sched, utc(MON, "20:00"))).toBeNull();
  });

  it("is null when the schedule never locks", () => {
    expect(nextWindow(undefined, utc(MON, "07:00"))).toBeNull();
    expect(nextWindow({ ...sched, paused: true }, utc(MON, "07:00"))).toBeNull();
  });

  it("where tagged and untagged open together, the tagged time is the one that runs", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "16:00", end: "19:00" },
          { start: "16:00", end: "17:00", time: "learning" },
        ],
      },
    };
    expect(nextWindow(s, utc(MON, "07:00"))?.time).toBe("learning");
  });

  it("can skip windows whose tag is not defined", () => {
    expect(nextWindow(sched, utc(MON, "07:00"), [quiet])?.time).toBeUndefined();
    expect(nextWindow(sched, utc(MON, "07:00"), [quiet])?.hhmm).toBe("16:00");
  });

  it("lands on the right instant across a DST change", () => {
    // Europe/London springs forward on Sun 2027-03-28 (01:00 UTC); a Monday
    // 09:00 window the day after is 08:00 UTC.
    const s: Schedule = { tz: "Europe/London", weekly: { sun: [{ start: "05:00", end: "06:00" }], mon: [{ start: "09:00", end: "10:00" }] } };
    const next = nextWindow(s, utc("2027-03-28", "10:00"));
    expect(next?.day).toBe("tomorrow");
    expect(next?.startsAt).toBe(utc("2027-03-29", "08:00"));
  });
});

describe("findTaggedOverlaps", () => {
  it("reports tagged∩tagged on weekly days and override dates, never tagged∩untagged", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "11:00", time: "a" },
          { start: "10:00", end: "12:00", time: "b" },
          { start: "10:30", end: "11:30" },
        ],
        tue: [
          { start: "09:00", end: "10:00", time: "a" },
          { start: "10:00", end: "11:00", time: "b" },
        ],
      },
      overrides: {
        "2026-12-25": [
          { start: "09:00", end: "11:00", time: "a" },
          { start: "09:00", end: "11:00", time: "a" },
        ],
      },
    };
    const found = findTaggedOverlaps(s);
    expect(found.map((o) => o.day)).toEqual(["mon", "2026-12-25"]);
  });
  it("is empty for no schedule", () => {
    expect(findTaggedOverlaps(undefined)).toEqual([]);
  });
});

// --- the gate -----------------------------------------------------------

function device(over: Partial<Device>): Device {
  return {
    id: "d1",
    label: "Phone",
    platform: "android",
    pairing: "paired",
    devicePubkey: "aa".repeat(32),
    ...over,
  } as Device;
}
function status(machine: string, appVersionCode: number): Record<string, DeviceStatus> {
  return {
    [machine]: {
      v: 1,
      subject: "cd".repeat(32),
      machine,
      ts: 1,
      dayKey: "2026-11-02",
      usedTodaySecs: 0,
      windowLeftSecs: 0,
      quotaLeftSecs: 0,
      effectiveSecs: 0,
      locked: false,
      source: "guardian",
      appVersionCode,
    },
  };
}
const phone = device({ id: "phone", platform: "android", devicePubkey: "aa".repeat(32) });
const laptop = device({ id: "laptop", platform: "linux", label: "Laptop", devicePubkey: "bb".repeat(32) });

describe("canUseTimes / wardenSupport.times", () => {
  it("is Android 49 and charterd 800", () => {
    expect(canUseTimes(phone, status(phone.devicePubkey!, 49))).toBe(true);
    expect(canUseTimes(phone, status(phone.devicePubkey!, 48))).toBe(false);
    expect(canUseTimes(laptop, status(laptop.devicePubkey!, 800))).toBe(true);
    expect(canUseTimes(laptop, status(laptop.devicePubkey!, 712))).toBe(false);
    expect(wardenSupport([phone], status(phone.devicePubkey!, 48), "times").tooOld).toEqual([phone]);
  });
  it("a device that has not reported is assumed capable (house doctrine: silence is not incapacity)", () => {
    expect(canUseTimes(phone)).toBe(true);
    expect(canUseTimes(laptop, {})).toBe(true);
  });
});

// --- editing ------------------------------------------------------------

const base: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }], tue: [{ start: "16:00", end: "19:00" }] } },
  times: [learning, quiet],
};
const both = [phone, laptop];
const bothNew = { ...status(phone.devicePubkey!, 49), ...status(laptop.devicePubkey!, 800) };

describe("setTimeWindows", () => {
  it("splits a shared schedule first, seeding EVERY device, so siblings keep untagged windows", () => {
    const out = setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "12:00" }] }, both, bothNew);
    // The base is untouched ...
    expect(out.schedule).toEqual(base.schedule);
    // ... the sibling got the shared value, untagged ...
    expect(out.deviceOverrides!.phone.schedule).toEqual(base.schedule);
    // ... and only the laptop carries the tagged window.
    expect(out.deviceOverrides!.laptop.schedule!.weekly.mon).toEqual([
      { start: "16:00", end: "19:00" },
      { start: "09:00", end: "12:00", time: "learning" },
    ]);
    // The wire proves it: no tag reaches the phone.
    const phoneWire = policyToClauses(null, effectivePolicyForDevice(out, "phone"), 1);
    const scheduleOf = (w: typeof phoneWire) => JSON.stringify(w.find((c) => c.kind === "schedule"));
    expect(scheduleOf(phoneWire)).not.toContain('"time"');
    const laptopWire = policyToClauses(null, effectivePolicyForDevice(out, "laptop"), 1);
    expect(scheduleOf(laptopWire)).toContain('"time":"learning"');
  });

  it("splits even when the schedule is already split for OTHER devices only", () => {
    const partly: Policy = { ...base, deviceOverrides: { phone: { schedule: base.schedule } } };
    const out = setTimeWindows(partly, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    expect(out.schedule).toEqual(base.schedule); // the base never takes the tag
    expect(out.deviceOverrides!.phone.schedule).toEqual(base.schedule);
    expect(out.deviceOverrides!.laptop.schedule!.weekly.mon!.some((w) => w.time === "learning")).toBe(true);
  });

  it("replaces only this time's weekly windows on this device", () => {
    const first = setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }], tue: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    const second = setTimeWindows(first, "learning", "laptop", { tue: [{ start: "11:00", end: "12:00" }] }, both, bothNew);
    const w = second.deviceOverrides!.laptop.schedule!.weekly;
    expect(w.mon).toEqual([{ start: "16:00", end: "19:00" }]); // Monday's Learning window went; Free stayed
    expect(w.tue).toEqual([
      { start: "16:00", end: "19:00" },
      { start: "11:00", end: "12:00", time: "learning" },
    ]);
  });

  it("leaves other times' windows and date overrides alone", () => {
    const seeded: Policy = {
      ...base,
      deviceOverrides: {
        laptop: {
          schedule: {
            tz: "UTC",
            weekly: { mon: [{ start: "07:00", end: "08:00", time: "quiet" }] },
            overrides: { "2026-12-25": [{ start: "09:00", end: "10:00", time: "learning" }] },
          },
        },
      },
    };
    const out = setTimeWindows(seeded, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    const s = out.deviceOverrides!.laptop.schedule!;
    expect(s.weekly.mon).toEqual([
      { start: "07:00", end: "08:00", time: "quiet" },
      { start: "09:00", end: "10:00", time: "learning" },
    ]);
    expect(s.overrides!["2026-12-25"][0].time).toBe("learning");
  });

  it("does not mutate its input", () => {
    const snapshot = structuredClone(base);
    setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    expect(base).toEqual(snapshot);
  });

  it("REFUSES a device that fails the times gate, changing nothing", () => {
    const old = { ...bothNew, ...status(laptop.devicePubkey!, 712) };
    expect(() => setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, old)).toThrow(
      /latest Kintrinsic/,
    );
  });

  it("refuses an undefined time, an unknown device, a bad window, an overlap, no schedule, a lifted schedule", () => {
    const w = { mon: [{ start: "09:00", end: "10:00" }] };
    expect(() => setTimeWindows(base, "ghost", "laptop", w, both)).toThrow(/no time/i);
    expect(() => setTimeWindows(base, "free", "laptop", w, both)).toThrow();
    expect(() => setTimeWindows(base, "learning", "nope", w, both)).toThrow(/isn't one of/i);
    expect(() => setTimeWindows(base, "learning", "laptop", { mon: [{ start: "10:00", end: "09:00" }] }, both)).toThrow(/start before/);
    expect(() => setTimeWindows(base, "learning", "laptop", { mon: [{ start: "9:00", end: "10:00" }] }, both)).toThrow();
    const withQuiet = setTimeWindows(base, "quiet", "laptop", { mon: [{ start: "09:00", end: "11:00" }] }, both, bothNew);
    expect(() => setTimeWindows(withQuiet, "learning", "laptop", { mon: [{ start: "10:00", end: "12:00" }] }, both, bothNew)).toThrow(
      /overlap/i,
    );
    expect(() => setTimeWindows({ ...base, schedule: undefined }, "learning", "laptop", w, both)).toThrow(/schedule/i);
    expect(() =>
      setTimeWindows({ ...base, schedule: { ...base.schedule!, paused: true } }, "learning", "laptop", w, both),
    ).toThrow(/switched off/i);
  });

  it("an empty window set clears the time's windows on that device", () => {
    const first = setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    const cleared = setTimeWindows(first, "learning", "laptop", {}, both, bothNew);
    expect(cleared.deviceOverrides!.laptop.schedule!.weekly.mon).toEqual([{ start: "16:00", end: "19:00" }]);
  });
});

describe("removeNamedTime", () => {
  const withWindows: Policy = {
    ...base,
    schedule: {
      tz: "UTC",
      weekly: { mon: [{ start: "09:00", end: "10:00", time: "learning" }, { start: "16:00", end: "19:00" }], wed: [{ start: "09:00", end: "10:00", time: "learning" }] },
      overrides: { "2026-12-25": [{ start: "09:00", end: "10:00", time: "learning" }], "2026-12-26": [{ start: "09:00", end: "10:00", time: "quiet" }] },
    },
    deviceOverrides: {
      laptop: { schedule: { tz: "UTC", weekly: { tue: [{ start: "09:00", end: "10:00", time: "learning" }, { start: "10:00", end: "11:00", time: "quiet" }] } } },
      phone: { budget: { tz: "UTC", dailyMinutes: 30 } },
    },
  };

  it("drops the time and strips its tagged windows from the base AND every device schedule", () => {
    const out = removeNamedTime(withWindows, "learning");
    expect(out.times).toEqual([quiet]);
    expect(out.schedule!.weekly).toEqual({ mon: [{ start: "16:00", end: "19:00" }] }); // wed emptied → dropped
    expect(out.schedule!.overrides).toEqual({
      "2026-12-25": [], // kept EMPTY: a closed day, not a missing one
      "2026-12-26": [{ start: "09:00", end: "10:00", time: "quiet" }],
    });
    expect(out.deviceOverrides!.laptop.schedule!.weekly.tue).toEqual([{ start: "10:00", end: "11:00", time: "quiet" }]);
    expect(unknownTimeTags(out)).toEqual([]);
    // An override for another control is untouched.
    expect(out.deviceOverrides!.phone).toEqual(withWindows.deviceOverrides!.phone);
  });

  it("deleting the last time leaves a real empty set (so the clause clears)", () => {
    const only: Policy = { ...withWindows, times: [learning] };
    expect(removeNamedTime(only, "learning").times).toEqual([]);
  });

  it("does not mutate, and leaves an untouched policy as it was", () => {
    const snap = structuredClone(withWindows);
    removeNamedTime(withWindows, "learning");
    expect(withWindows).toEqual(snap);
    expect(removeNamedTime(base, "nothing-here")).toEqual(base);
  });

  it("refuses to delete Free time", () => {
    expect(() => removeNamedTime(base, "free")).toThrow(/Free time/);
  });

  it("an orphaned tag is reported, and is exactly what removal prevents", () => {
    const orphaned: Policy = { ...withWindows, times: [quiet] };
    expect(unknownTimeTags(orphaned)).toEqual(["learning"]);
  });
});

describe("the set: add, update, rename", () => {
  it("adds, refusing a duplicate id, an invalid time and the 17th", () => {
    const p: Policy = { id: "p", scope: { kind: "device" } };
    const one = addNamedTime(p, learning);
    expect(one.times).toEqual([learning]);
    expect(() => addNamedTime(one, learning)).toThrow(/already/);
    expect(() => addNamedTime(p, { ...learning, id: "free" })).toThrow();
    const full: Policy = { ...p, times: Array.from({ length: 16 }, (_, i) => ({ ...learning, id: `t${i}` })) };
    expect(() => addNamedTime(full, learning)).toThrow(/at most 16/);
  });

  it("update replaces by id and refuses an unknown one", () => {
    const out = updateNamedTime(base, { ...learning, apps: ["x.y"] });
    expect(out.times![0].apps).toEqual(["x.y"]);
    expect(() => updateNamedTime(base, { ...learning, id: "ghost" })).toThrow();
  });

  it("rename changes the label ONLY — the id and every tagged window stay put", () => {
    const tagged = setTimeWindows(base, "learning", "laptop", { mon: [{ start: "09:00", end: "10:00" }] }, both, bothNew);
    const renamed = renameNamedTime(tagged, "learning", "  Homework  ");
    expect(renamed.times![0]).toEqual({ ...learning, label: "Homework" });
    expect(renamed.times![0].id).toBe("learning");
    expect(renamed.deviceOverrides).toEqual(tagged.deviceOverrides);
    expect(unknownTimeTags(renamed)).toEqual([]);
    expect(() => renameNamedTime(base, "learning", " ")).toThrow();
    expect(() => renameNamedTime(base, "learning", "x".repeat(61))).toThrow();
    expect(() => renameNamedTime(base, "ghost", "X")).toThrow();
  });
});

describe("times is a per-child definition, not a splittable control", () => {
  it("is not in SPLITTABLE_CONTROLS and reaches every device unchanged", () => {
    expect((SPLITTABLE_CONTROLS as readonly string[]).includes("times")).toBe(false);
    const split: Policy = { ...base, deviceOverrides: { laptop: { schedule: base.schedule } } };
    expect(effectivePolicyForDevice(split, "laptop").times).toEqual(base.times);
    expect(effectivePolicyForDevice(split, "phone").times).toEqual(base.times);
  });
});

describe("templates", () => {
  it("learningTimeTemplate is only-these, empty, and not counted against the budget", () => {
    expect(learningTimeTemplate()).toEqual({
      id: "learning",
      label: "Learning time",
      posture: "onlyThese",
      apps: [],
      sites: [],
      countsAgainstBudget: false,
    });
  });

  it("learningTimeTemplate can be seeded with sites; unseeded it refuses to be saved", () => {
    const seeded = learningTimeTemplate([
      { id: "khan-academy", label: "Khan Academy", kind: "site", url: "https://www.khanacademy.org/", domains: ["khanacademy.org"] },
    ]);
    expect(namedTimeProblems(seeded)).toEqual([]);
    expect(namedTimeProblems(learningTimeTemplate())).toEqual(["Choose at least one app or website for this time."]);
  });

  it("customTimeTemplate slugs the label and de-duplicates against what's taken", () => {
    expect(customTimeTemplate("Quiet Reading!").id).toBe("quiet-reading");
    expect(customTimeTemplate("Quiet Reading", ["quiet-reading"]).id).toBe("quiet-reading-2");
    expect(customTimeTemplate("Quiet Reading", ["quiet-reading", "quiet-reading-2"]).id).toBe("quiet-reading-3");
    expect(customTimeTemplate("  Quiet  ").label).toBe("Quiet");
    expect(customTimeTemplate("Quiet").posture).toBe("onlyThese");
  });

  it("slugs are valid ids: never 'free', never empty, never over 32 or hyphen-edged", () => {
    expect(timeSlug("Free")).toBe("free-2");
    expect(timeSlug("!!!")).toBe("time");
    expect(timeSlug("日本語")).toBe("time");
    const long = timeSlug("a".repeat(60));
    expect(long).toBe("a".repeat(32));
    const dup = timeSlug("a".repeat(60), [long]);
    expect(dup.length).toBeLessThanOrEqual(32);
    expect(dup).toMatch(/^[a-z0-9]+(-[a-z0-9]+)*$/);
    expect(timeSlug("x".repeat(31) + " y")).toMatch(/^[a-z0-9-]{1,32}$/);
    expect(timeSlug("x".repeat(31) + " y")).not.toMatch(/-$/);
  });

  it("namedTimeProblems names what stops a save", () => {
    expect(namedTimeProblems({ ...learning, label: " " })).toContain("Give this time a name.");
    expect(namedTimeProblems({ ...learning, apps: [], sites: [] })).toContain("Choose at least one app or website for this time.");
    const site = { id: "s", label: "S", kind: "site" as const, url: "https://s.example", domains: ["s.example"] };
    expect(namedTimeProblems({ ...learning, sites: [site, site] })).toContain("The same website is listed twice.");
    // Everything-except needs nothing listed: it is Free time plus departures.
    expect(namedTimeProblems({ id: "x", label: "X", posture: "everythingExcept" })).toEqual([]);
    expect(namedTimeProblems(learning)).toEqual([]);
  });
});
