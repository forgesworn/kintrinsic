// The `times` clause on the guardian wire (contract §Times): validation,
// emission, read-back, the tag on schedule windows, and the `time` field on
// asks, grants and STATUS. The device treats a bad body as ABSENT (every tagged
// window then locks), so the send side must throw rather than ship one.

import { describe, expect, it } from "vitest";
import type { NamedTime, Policy, Schedule } from "../domain/types";
import { buildTimeExtendGrant, MAX_NAMED_TIMES, MAX_TIME_LABEL_LEN } from "./grant";
import { parseRequest } from "./request";
import { parseStatus } from "./status";
import { policyToClauses, scheduleToGrant, timesToGrant } from "./clause";
import {
  FREE_TIME_ID,
  isValidTimeId,
  namedTimeFault,
  normaliseNamedTime,
  parseGrantTimes,
  siteFault,
  timesFault,
} from "./times";
import type { LearningApp } from "./types";

const khan: LearningApp = {
  id: "khan-academy",
  label: "Khan Academy",
  kind: "site",
  url: "https://www.khanacademy.org/",
  domains: ["khanacademy.org", "kastatic.org"],
};

const learning: NamedTime = {
  id: "learning",
  label: "Learning time",
  posture: "onlyThese",
  apps: ["org.example.maths"],
  sites: [khan],
};

const AT = 1_700_000_000;

describe("id and label rules", () => {
  it("accepts [a-z0-9-]{1,32} and refuses 'free', case, spaces, 33 chars, empties", () => {
    expect(isValidTimeId("learning")).toBe(true);
    expect(isValidTimeId("a-1")).toBe(true);
    expect(isValidTimeId("a".repeat(32))).toBe(true);
    expect(isValidTimeId("a".repeat(33))).toBe(false);
    expect(isValidTimeId("")).toBe(false);
    expect(isValidTimeId("Learning")).toBe(false);
    expect(isValidTimeId("my time")).toBe(false);
    expect(isValidTimeId("free")).toBe(false);
    expect(FREE_TIME_ID).toBe("free");
    expect(isValidTimeId(7)).toBe(false);
  });

  it("bounds the label at 1..60 characters, counting characters not code units", () => {
    expect(namedTimeFault({ ...learning, label: "" })).not.toBeNull();
    expect(namedTimeFault({ ...learning, label: "x".repeat(MAX_TIME_LABEL_LEN) })).toBeNull();
    expect(namedTimeFault({ ...learning, label: "x".repeat(MAX_TIME_LABEL_LEN + 1) })).not.toBeNull();
    // 60 emoji is 120 UTF-16 units but 60 characters: the Rust side counts chars.
    expect(namedTimeFault({ ...learning, label: "📚".repeat(60) })).toBeNull();
    expect(namedTimeFault({ ...learning, label: "📚".repeat(61) })).not.toBeNull();
  });

  it("refuses an unknown posture and mistyped fields", () => {
    expect(namedTimeFault({ ...learning, posture: "both" })).not.toBeNull();
    expect(namedTimeFault({ ...learning, apps: "maths" })).not.toBeNull();
    expect(namedTimeFault({ ...learning, apps: [1] })).not.toBeNull();
    expect(namedTimeFault({ ...learning, countsAgainstBudget: "yes" })).not.toBeNull();
    expect(namedTimeFault(null)).not.toBeNull();
  });
});

describe("site validation (the learning clause's rules, site kind only)", () => {
  it("accepts a pinned site", () => {
    expect(siteFault(khan)).toBeNull();
  });
  it("refuses a native app, a missing url, empty domains, a bad slug, no name", () => {
    expect(siteFault({ ...khan, kind: "native", exec: "/usr/bin/x" })).not.toBeNull();
    expect(siteFault({ ...khan, url: undefined })).not.toBeNull();
    expect(siteFault({ ...khan, url: " " })).not.toBeNull();
    expect(siteFault({ ...khan, domains: [] })).not.toBeNull();
    expect(siteFault({ ...khan, domains: undefined })).not.toBeNull();
    expect(siteFault({ ...khan, domains: [""] })).not.toBeNull();
    expect(siteFault({ ...khan, id: "Khan Academy" })).not.toBeNull();
    expect(siteFault({ ...khan, label: "" })).not.toBeNull();
  });
  it("is enforced through the time that carries it", () => {
    expect(namedTimeFault({ ...learning, sites: [{ ...khan, domains: [] }] })).not.toBeNull();
  });
});

describe("the whole set", () => {
  const mk = (n: number): NamedTime[] =>
    Array.from({ length: n }, (_, i) => ({ id: `t${i}`, label: `T${i}`, posture: "onlyThese", apps: ["a.b"] }));

  it("allows up to MAX_NAMED_TIMES, not one more", () => {
    expect(MAX_NAMED_TIMES).toBe(16);
    expect(timesFault(mk(16))).toBeNull();
    expect(timesFault(mk(17))).not.toBeNull();
  });
  it("refuses duplicate ids and a non-list", () => {
    expect(timesFault([learning, { ...learning, label: "Other" }])).toMatch(/share the id/);
    expect(timesFault("nope")).not.toBeNull();
  });
  it("an empty set is valid — it is how the last time is deleted", () => {
    expect(timesFault([])).toBeNull();
  });
});

describe("normaliseNamedTime", () => {
  it("keeps only the fields that belong to the posture", () => {
    const messy: NamedTime = {
      id: "x",
      label: "  X  ",
      posture: "onlyThese",
      apps: [" a.b ", "a.b", ""],
      blockApps: ["c.d"],
      denySites: ["evil.com"],
    };
    expect(normaliseNamedTime(messy)).toEqual({ id: "x", label: "X", posture: "onlyThese", apps: ["a.b"] });
    expect(
      normaliseNamedTime({
        id: "y",
        label: "Y",
        posture: "everythingExcept",
        blockApps: ["c.d"],
        apps: ["a.b"],
        sites: [khan],
      }),
    ).toEqual({ id: "y", label: "Y", posture: "everythingExcept", blockApps: ["c.d"] });
  });
  it("emits countsAgainstBudget only when true", () => {
    expect("countsAgainstBudget" in normaliseNamedTime({ ...learning, countsAgainstBudget: false })).toBe(false);
    expect(normaliseNamedTime({ ...learning, countsAgainstBudget: true }).countsAgainstBudget).toBe(true);
  });
  it("carries a site as the pinned-window shape only", () => {
    const out = normaliseNamedTime({ ...learning, sites: [{ ...khan, free: false, trusted: true }] });
    expect(out.sites).toEqual([khan]);
  });
});

describe("timesToGrant / parseGrantTimes", () => {
  it("builds the replace-the-set body and reads it straight back", () => {
    const set: NamedTime[] = [
      learning,
      { id: "quiet", label: "Quiet reading", posture: "everythingExcept", blockApps: ["a.b"], undenySites: ["wikipedia.org"] },
    ];
    const g = timesToGrant(set, AT);
    expect(g).toEqual({ v: 1, times: set, issuedAt: AT });
    expect(parseGrantTimes(JSON.parse(JSON.stringify(g)))).toEqual(g);
  });

  it("round-trips an empty clear", () => {
    const g = timesToGrant([], AT);
    expect(g).toEqual({ v: 1, times: [], issuedAt: AT });
    expect(parseGrantTimes(g)).toEqual(g);
  });

  it("trims a label before checking it", () => {
    expect(timesToGrant([{ ...learning, label: "  Learning time " }], AT).times[0].label).toBe("Learning time");
    expect(() => timesToGrant([{ ...learning, label: "   " }], AT)).toThrow(/can't be sent/);
  });

  it("THROWS rather than sending a body the device would drop", () => {
    expect(() => timesToGrant([{ ...learning, id: "free" }], AT)).toThrow(/reserved/);
    expect(() => timesToGrant([{ ...learning, id: "Has Space" }], AT)).toThrow();
    expect(() => timesToGrant([learning, learning], AT)).toThrow(/share the id/);
    expect(() => timesToGrant(Array.from({ length: 17 }, (_, i) => ({ ...learning, id: `t${i}` })), AT)).toThrow();
    expect(() => timesToGrant([{ ...learning, sites: [{ ...khan, kind: "native" }] }], AT)).toThrow();
  });

  it("an only-these time with nothing in it is valid on the wire (it runs nothing)", () => {
    expect(() => timesToGrant([{ id: "empty", label: "Empty", posture: "onlyThese" }], AT)).not.toThrow();
  });

  it("parseGrantTimes is fail-closed: any fault, wrong v, or bad issuedAt gives null", () => {
    const ok = timesToGrant([learning], AT);
    expect(parseGrantTimes({ ...ok, v: 2 })).toBeNull();
    expect(parseGrantTimes({ ...ok, issuedAt: -1 })).toBeNull();
    expect(parseGrantTimes({ ...ok, issuedAt: "now" })).toBeNull();
    expect(parseGrantTimes({ ...ok, times: [learning, { ...learning, id: "free" }] })).toBeNull();
    expect(parseGrantTimes({ ...ok, times: "x" })).toBeNull();
    expect(parseGrantTimes(null)).toBeNull();
    expect(parseGrantTimes("nope")).toBeNull();
  });
});

describe("policyToClauses and the times clause", () => {
  const schedule: Schedule = {
    tz: "Europe/London",
    weekly: { mon: [{ start: "16:00", end: "19:00" }], tue: [{ start: "16:00", end: "19:00" }] },
    overrides: { "2026-12-25": [{ start: "10:00", end: "20:00" }] },
  };
  const base: Policy = {
    id: "p1",
    scope: { kind: "device" },
    schedule,
    budget: { tz: "Europe/London", dailyMinutes: 120 },
  };

  it("a child with no times is BYTE-IDENTICAL on the wire (pinned literal)", () => {
    const out = policyToClauses("ab".repeat(32), base, AT);
    expect(out.map((c) => c.kind)).toEqual(["schedule", "budget"]);
    expect(JSON.stringify(out)).toBe(
      JSON.stringify([
        {
          v: 1,
          kind: "schedule",
          issuedAt: AT,
          body: {
            v: 1,
            tz: "Europe/London",
            weekly: {
              mon: [{ start: "16:00", end: "19:00" }],
              tue: [{ start: "16:00", end: "19:00" }],
            },
            issuedAt: AT,
            overrides: { "2026-12-25": [{ start: "10:00", end: "20:00" }] },
          },
          subject: "ab".repeat(32),
        },
        {
          v: 1,
          kind: "budget",
          issuedAt: AT,
          body: { v: 1, tz: "Europe/London", issuedAt: AT, dailyMinutes: 120 },
          subject: "ab".repeat(32),
        },
      ]),
    );
    expect(JSON.stringify(out)).not.toContain("time");
  });

  it("emits the times clause first, with the child's subject", () => {
    const out = policyToClauses("ab".repeat(32), { ...base, times: [learning] }, AT);
    expect(out.map((c) => c.kind)).toEqual(["times", "schedule", "budget"]);
    expect(out[0]).toEqual({
      v: 1,
      kind: "times",
      issuedAt: AT,
      body: { v: 1, times: [learning], issuedAt: AT },
      subject: "ab".repeat(32),
    });
  });

  it("an explicit empty set signs a real clear; undefined signs nothing", () => {
    expect(policyToClauses(null, { ...base, times: [] }, AT)[0]).toMatchObject({
      kind: "times",
      body: { times: [] },
    });
    expect(policyToClauses(null, { ...base, times: undefined }, AT).some((c) => c.kind === "times")).toBe(false);
  });

  it("an app-scope policy still yields nothing", () => {
    expect(policyToClauses(null, { id: "a", scope: { kind: "app", appId: "x.y", label: "X" }, times: [learning] }, AT)).toEqual([]);
  });
});

describe("tagged schedule windows", () => {
  it("carry `time` on the wire only when set", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "12:00", time: "learning" },
          { start: "16:00", end: "19:00" },
        ],
      },
      overrides: { "2026-12-25": [{ start: "09:00", end: "10:00", time: "learning" }] },
    };
    const g = scheduleToGrant(s, AT);
    expect(g.weekly.mon).toEqual([
      { start: "09:00", end: "12:00", time: "learning" },
      { start: "16:00", end: "19:00" },
    ]);
    expect(Object.keys(g.weekly.mon![1])).toEqual(["start", "end"]); // never `time: undefined`
    expect(g.overrides!["2026-12-25"][0].time).toBe("learning");
  });

  it("a tagged window over an untagged one is a carve, and is fine", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "16:00", end: "19:00" },
          { start: "17:00", end: "18:00", time: "learning" },
        ],
      },
    };
    expect(() => scheduleToGrant(s, AT)).not.toThrow();
  });

  it("two tagged windows overlapping on a day is refused at the source", () => {
    const weekly: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "11:00", time: "a" },
          { start: "10:00", end: "12:00", time: "b" },
        ],
      },
    };
    expect(() => scheduleToGrant(weekly, AT)).toThrow(/overlap on mon/);
    const dated: Schedule = {
      tz: "UTC",
      weekly: { mon: [{ start: "09:00", end: "10:00" }] },
      overrides: {
        "2026-12-25": [
          { start: "09:00", end: "11:00", time: "a" },
          { start: "09:30", end: "10:00", time: "a" },
        ],
      },
    };
    expect(() => scheduleToGrant(dated, AT)).toThrow(/overlap on 2026-12-25/);
  });

  it("touching tagged windows are not an overlap", () => {
    const s: Schedule = {
      tz: "UTC",
      weekly: {
        mon: [
          { start: "09:00", end: "10:00", time: "a" },
          { start: "10:00", end: "11:00", time: "b" },
        ],
      },
    };
    expect(() => scheduleToGrant(s, AT)).not.toThrow();
  });

  it("refuses a malformed or reserved tag; an UNDEFINED id is allowed (it just locks)", () => {
    const tag = (time: string): Schedule => ({ tz: "UTC", weekly: { mon: [{ start: "09:00", end: "10:00", time }] } });
    expect(() => scheduleToGrant(tag("Bad Tag"), AT)).toThrow(/named-time id/);
    expect(() => scheduleToGrant(tag("free"), AT)).toThrow(/named-time id/);
    expect(() => scheduleToGrant(tag("not-defined-yet"), AT)).not.toThrow();
  });

  it("a lifted schedule drops tagged windows with the rest", () => {
    const g = scheduleToGrant(
      { tz: "UTC", paused: true, weekly: { mon: [{ start: "09:00", end: "10:00", time: "a" }] } },
      AT,
    );
    expect(g.weekly).toEqual({});
  });
});

describe("time on asks, grants and STATUS", () => {
  const ask = (params: Record<string, unknown>) =>
    JSON.stringify({
      v: 1,
      op: "time.extend",
      reqId: "ab".repeat(32),
      nonce: "cd".repeat(32),
      subject: "ef".repeat(32),
      machine: "12".repeat(32),
      ts: AT,
      params: { minutesRequested: 30, limitHit: "budget", ...params },
    });

  it("parseRequest reads a valid `time` and leaves an absent one absent", () => {
    expect(parseRequest(ask({ time: "learning" }))?.params).toMatchObject({ time: "learning" });
    expect((parseRequest(ask({})) as { params: { time?: string } }).params.time).toBeUndefined();
  });

  it("parseRequest drops an ask with a malformed or reserved `time`", () => {
    expect(parseRequest(ask({ time: "Bad Id" }))).toBeNull();
    expect(parseRequest(ask({ time: "free" }))).toBeNull();
    expect(parseRequest(ask({ time: 5 }))).toBeNull();
  });

  const decision = {
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    minutesGranted: 15,
    limitHit: "schedule" as const,
    ts: AT,
    tz: "UTC",
  };

  it("buildTimeExtendGrant echoes time on allow and deny, and omits it when absent", () => {
    expect(buildTimeExtendGrant({ ...decision, decision: "allow", time: "learning" }).params).toEqual({
      minutesGranted: 15,
      limitHit: "schedule",
      time: "learning",
    });
    expect(buildTimeExtendGrant({ ...decision, decision: "deny", time: "learning" }).params).toEqual({
      minutesGranted: 0,
      limitHit: "schedule",
      time: "learning",
    });
    const plain = buildTimeExtendGrant({ ...decision, decision: "allow" }).params;
    expect(plain).toEqual({ minutesGranted: 15, limitHit: "schedule" });
    expect("time" in plain).toBe(false);
  });

  it("a 0-minute allow is a legal grant (the closing half of an other-time answer)", () => {
    const g = buildTimeExtendGrant({ ...decision, decision: "allow", minutesGranted: 0 });
    expect(g.decision).toBe("allow");
    expect(g.params.minutesGranted).toBe(0);
  });

  const status = (extra: Record<string, unknown>) =>
    JSON.stringify({
      v: 1,
      subject: "ab".repeat(32),
      machine: "cd".repeat(32),
      ts: AT,
      dayKey: "2026-10-30",
      usedTodaySecs: 0,
      windowLeftSecs: 0,
      quotaLeftSecs: 0,
      effectiveSecs: 0,
      locked: false,
      source: "guardian",
      ...extra,
    });

  it("STATUS activeTime parses when well-formed, and is absent otherwise", () => {
    expect(parseStatus(status({ activeTime: "learning" }))?.activeTime).toBe("learning");
    expect(parseStatus(status({}))?.activeTime).toBeUndefined();
    // A junk id costs the field, never the whole report.
    expect(parseStatus(status({ activeTime: "Bad Id" }))?.activeTime).toBeUndefined();
    expect(parseStatus(status({ activeTime: "free" }))?.activeTime).toBeUndefined();
    expect(parseStatus(status({ activeTime: 3 }))).not.toBeNull();
  });
});
