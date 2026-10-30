import { describe, expect, it } from "vitest";
import type { Device, NamedTime, Policy, Schedule } from "../domain/types";
import { timeEditorTitle } from "./timesView";
import {
  devicesLeftWithoutHours,
  hasTaggedWindows,
  postureSummary,
  timeCardSummary,
  windowsOnDevice,
  windowsSummary,
} from "./timesView";

const khan = { id: "khan-academy", label: "Khan Academy", kind: "site" as const, url: "https://khanacademy.org/", domains: ["khanacademy.org"] };
const youtube = { id: "youtube", label: "YouTube", kind: "site" as const, url: "https://youtube.com/", domains: ["youtube.com"] };

const LAPTOP: Device = { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) };
const PHONE: Device = { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: "b".repeat(64) };

const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", sites: [khan, youtube] };

describe("windowsSummary", () => {
  it("folds runs of days with the same hours", () => {
    const w = (s: string, e: string) => [{ start: s, end: e }];
    expect(
      windowsSummary({ mon: w("09:00", "11:00"), tue: w("09:00", "11:00"), wed: w("09:00", "11:00"), thu: w("09:00", "11:00"), fri: w("09:00", "11:00") }),
    ).toBe("Mon–Fri 09:00–11:00");
  });

  it("says every day, a pair, and a lone day plainly, and joins different runs", () => {
    const w = (s: string, e: string) => [{ start: s, end: e }];
    const every = Object.fromEntries(["mon", "tue", "wed", "thu", "fri", "sat", "sun"].map((d) => [d, w("08:00", "09:00")]));
    expect(windowsSummary(every)).toBe("Every day 08:00–09:00");
    expect(windowsSummary({ sat: w("10:00", "12:00"), sun: w("10:00", "12:00") })).toBe("Sat, Sun 10:00–12:00");
    expect(windowsSummary({ mon: w("09:00", "10:00"), wed: w("15:00", "16:00") })).toBe("Mon 09:00–10:00 · Wed 15:00–16:00");
  });

  it("two windows on one day stay on that day", () => {
    expect(
      windowsSummary({
        mon: [
          { start: "09:00", end: "10:00" },
          { start: "14:00", end: "15:00" },
        ],
      }),
    ).toBe("Mon 09:00–10:00, 14:00–15:00");
  });

  it("is honest when there are none", () => {
    expect(windowsSummary({})).toBe("No hours yet");
  });
});

describe("what a time says it allows", () => {
  it("names one or two websites, counts more, and adds apps", () => {
    expect(postureSummary(LEARNING)).toBe("Only Khan Academy, YouTube");
    expect(postureSummary({ ...LEARNING, sites: [khan, youtube, { ...khan, id: "b" }], apps: ["a", "b"] })).toBe(
      "Only 3 websites and 2 apps",
    );
  });

  it("says plainly that an only-these time with nothing in it has nothing chosen", () => {
    expect(postureSummary({ id: "x", label: "X", posture: "onlyThese" })).toBe("Only these — nothing chosen yet");
  });

  it("describes an everything-except time by its departures from Free time", () => {
    expect(postureSummary({ id: "c", label: "Chores", posture: "everythingExcept" })).toBe("Everything Free time allows");
    expect(postureSummary({ id: "c", label: "Chores", posture: "everythingExcept", blockApps: ["roblox"], undenySites: ["youtube.com"] })).toBe(
      "Everything Free time allows, with 2 changes",
    );
  });

  it("says whether it uses up screen time", () => {
    expect(timeCardSummary(LEARNING)).toContain("Doesn’t use up screen time");
    expect(timeCardSummary({ ...LEARNING, countsAgainstBudget: true })).toContain("Uses up screen time");
  });
});

const sched = (extra: Partial<Schedule["weekly"]> = {}): Schedule => ({
  tz: "UTC",
  weekly: { mon: [{ start: "16:00", end: "18:00" }], ...extra },
});

describe("per-device hours", () => {
  const policy: Policy = {
    id: "p",
    scope: { kind: "device" },
    times: [LEARNING],
    schedule: sched(),
    deviceOverrides: {
      d1: { schedule: sched({ tue: [{ start: "09:00", end: "11:00", time: "learning" }] }) },
      d2: { schedule: sched() },
    },
  };

  it("reads a time's hours from that device's own schedule", () => {
    expect(windowsOnDevice(policy, "learning", "d1")).toEqual({ tue: [{ start: "09:00", end: "11:00" }] });
    expect(windowsOnDevice(policy, "learning", "d2")).toEqual({});
  });

  it("tells a tagged schedule from a plain one", () => {
    expect(hasTaggedWindows(policy.deviceOverrides!.d1.schedule)).toBe(true);
    expect(hasTaggedWindows(policy.schedule)).toBe(false);
    expect(hasTaggedWindows(undefined)).toBe(false);
  });
});

describe("devices left with nothing after a delete", () => {
  it("names a paired device whose schedule would have no windows at all", () => {
    const after: Policy = {
      id: "p",
      scope: { kind: "device" },
      schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
      deviceOverrides: { d1: { schedule: { tz: "UTC", weekly: { mon: [], tue: [] } } } },
    };
    expect(devicesLeftWithoutHours(after, [LAPTOP, PHONE]).map((d) => d.id)).toEqual(["d1"]);
  });

  it("does not count a lifted schedule, an override day with windows, or an unpaired device", () => {
    const after: Policy = {
      id: "p",
      scope: { kind: "device" },
      schedule: { tz: "UTC", weekly: {}, paused: true },
      deviceOverrides: { d2: { schedule: { tz: "UTC", weekly: {}, overrides: { "2026-12-25": [{ start: "10:00", end: "12:00" }] } } } },
    };
    expect(devicesLeftWithoutHours(after, [LAPTOP, PHONE])).toEqual([]);
    const empty: Policy = { id: "p", scope: { kind: "device" }, schedule: { tz: "UTC", weekly: {} } };
    expect(devicesLeftWithoutHours(empty, [{ ...LAPTOP, pairing: "pending" as Device["pairing"] }])).toEqual([]);
  });
});

describe("the editor's title", () => {
  const child = {
    id: "c1",
    name: "Sam",
    color: "#000",
    dependantPubkey: null,
    devices: [],
    policies: [{ id: "p", scope: { kind: "device" as const }, times: [LEARNING] }],
  };
  it("is the time's name, or what is about to be made", () => {
    expect(timeEditorTitle(child, { timeId: "learning" })).toBe("Learning time");
    expect(timeEditorTitle(child, { template: "learning" })).toBe("Learning time");
    expect(timeEditorTitle(child, { template: "custom", name: "Reading time" })).toBe("Reading time");
    expect(timeEditorTitle(child, { template: "custom" })).toBe("New time");
  });
});
