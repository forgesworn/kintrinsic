import { describe, expect, it } from "vitest";
import type { AppsPolicy, Policy, Schedule, Budget } from "../domain/types";
import type { GrantBudget, GrantSchedule } from "./types";
import { appRulesToGrant, appsToGrant, budgetToGrant, policyToClauses, scheduleToGrant } from "./clause";

const SUBJECT = "a".repeat(64);
const AT = 1_700_000_000;

const sched = (over: Partial<Schedule> = {}): Schedule => ({
  tz: "Europe/London",
  weekly: { mon: [{ start: "16:00", end: "18:00" }] },
  ...over,
});
const budg = (over: Partial<Budget> = {}): Budget => ({
  tz: "Europe/London",
  dailyMinutes: 90,
  ...over,
});
const devicePolicy = (over: Partial<Policy> = {}): Policy => ({
  id: "p1",
  scope: { kind: "device" },
  schedule: sched(),
  budget: budg(),
  ...over,
});

describe("scheduleToGrant", () => {
  it("maps weekly windows + tz and stamps v/issuedAt", () => {
    const g = scheduleToGrant(sched(), AT);
    expect(g.v).toBe(1);
    expect(g.issuedAt).toBe(AT);
    expect(g.tz).toBe("Europe/London");
    expect(g.weekly.mon).toEqual([{ start: "16:00", end: "18:00" }]);
    expect(g.paused).toBeUndefined();
  });

  it("INVERTS app-paused (lifted) to wire always-allowed, NOT block-all", () => {
    // app paused = "always allowed"; wire paused = "block all". Must translate.
    const g = scheduleToGrant(sched({ paused: true }), AT);
    expect(g.paused).not.toBe(true);
    expect(g.weekly).toEqual({}); // empty weekly = no-op = always allowed
  });

  it("encodes an all-days-blocked (zero-window) schedule as wire block-all", () => {
    // Parent turns every day off (not app-paused) = "no screen time at all".
    // An all-empty weekly must NOT go out as the device's always-allowed no-op;
    // it must be an explicit wire `paused: true` (block all), or it fails OPEN.
    const g = scheduleToGrant(sched({ weekly: {} }), AT);
    expect(g.paused).toBe(true);
    expect(g.weekly).toEqual({});
  });

  it("treats a present override window as allowed time (not block-all)", () => {
    const g = scheduleToGrant(
      sched({ weekly: {}, overrides: { "2026-07-01": [{ start: "09:00", end: "10:00" }] } }),
      AT,
    );
    expect(g.paused).not.toBe(true);
    expect(g.overrides?.["2026-07-01"]).toEqual([{ start: "09:00", end: "10:00" }]);
  });

  it("rejects an inverted window (start >= end)", () => {
    expect(() =>
      scheduleToGrant(sched({ weekly: { mon: [{ start: "18:00", end: "16:00" }] } }), AT),
    ).toThrow();
  });
});

describe("budgetToGrant", () => {
  it("maps caps + weekStart and stamps v/issuedAt", () => {
    const g = budgetToGrant(budg({ weekStart: "mon" }), AT);
    expect(g.v).toBe(1);
    expect(g.issuedAt).toBe(AT);
    expect(g.dailyMinutes).toBe(90);
    expect(g.weekStart).toBe("mon");
    expect(g.paused).toBeUndefined();
  });

  it("INVERTS app-paused (lifted) to an unconstrained budget, NOT quota-0", () => {
    const g = budgetToGrant(budg({ paused: true }), AT);
    expect(g.paused).not.toBe(true);
    expect(g.dailyMinutes ?? null).toBeNull();
    expect(g.weeklyMinutes ?? null).toBeNull();
  });

  it("rejects an out-of-range daily cap", () => {
    expect(() => budgetToGrant(budg({ dailyMinutes: 0 }), AT)).toThrow();
    expect(() => budgetToGrant(budg({ dailyMinutes: 99999 }), AT)).toThrow();
  });

  // 2026-08-06 "named costs" design: `model` must be absent for `session`
  // (the pre-existing, back-compat wire shape), and present ONLY for
  // `"named"` — never emitted just because the field happens to be set.
  describe("time model (absent = session, back-compat)", () => {
    it("omits `model` entirely when unset", () => {
      const g = budgetToGrant(budg(), AT);
      expect(g.model).toBeUndefined();
      expect("model" in g).toBe(false);
    });

    it("a session-model budget serialises BYTE-IDENTICAL to one with no model at all", () => {
      const withoutModel = budgetToGrant(budg(), AT);
      const withSessionModel = budgetToGrant(budg({ model: "session" }), AT);
      expect(JSON.stringify(withSessionModel)).toBe(JSON.stringify(withoutModel));
    });

    it("emits `model: 'named'` when the budget selects the named tier", () => {
      const g = budgetToGrant(budg({ model: "named" }), AT);
      expect(g.model).toBe("named");
    });

    it("carries a named model through the paused (lifted) inversion too", () => {
      const g = budgetToGrant(budg({ paused: true, model: "named" }), AT);
      expect(g.paused).not.toBe(true); // still the lifted inversion
      expect(g.model).toBe("named");
    });

    it("a paused session-model budget stays byte-identical to a paused budget with no model", () => {
      const withoutModel = budgetToGrant(budg({ paused: true }), AT);
      const withSessionModel = budgetToGrant(budg({ paused: true, model: "session" }), AT);
      expect(JSON.stringify(withSessionModel)).toBe(JSON.stringify(withoutModel));
    });
  });
});

describe("policyToClauses", () => {
  it("emits a schedule + budget CLAUSE for a device policy, carrying subject", () => {
    const clauses = policyToClauses(SUBJECT, devicePolicy(), AT);
    expect(clauses.map((c) => c.kind).sort()).toEqual(["budget", "schedule"]);
    for (const c of clauses) {
      expect(c.v).toBe(1);
      expect(c.subject).toBe(SUBJECT);
      expect(c.issuedAt).toBe(AT);
    }
    const s = clauses.find((c) => c.kind === "schedule")!.body as GrantSchedule;
    expect(s.weekly.mon).toEqual([{ start: "16:00", end: "18:00" }]);
    const b = clauses.find((c) => c.kind === "budget")!.body as GrantBudget;
    expect(b.dailyMinutes).toBe(90);
  });

  it("emits only the present dimension", () => {
    expect(policyToClauses(SUBJECT, devicePolicy({ budget: undefined }), AT).map((c) => c.kind)).toEqual([
      "schedule",
    ]);
    expect(policyToClauses(SUBJECT, devicePolicy({ schedule: undefined }), AT).map((c) => c.kind)).toEqual([
      "budget",
    ]);
  });

  it("does NOT emit screen-time clauses for an app-scope policy", () => {
    const appPolicy = devicePolicy({ scope: { kind: "app", appId: "x", label: "X" } });
    expect(policyToClauses(SUBJECT, appPolicy, AT)).toEqual([]);
  });

  it("omits subject when null (single-child default)", () => {
    const clauses = policyToClauses(null, devicePolicy(), AT);
    expect(clauses[0].subject).toBeUndefined();
  });
});

describe("appRulesToGrant", () => {
  const appPolicy = (over: Partial<Policy> = {}): Policy => ({
    id: "pa",
    scope: { kind: "app", appId: "com.example.game", label: "Game" },
    ...over,
  });

  it("aggregates app-scope policies into one rule set (block flag carried)", () => {
    const g = appRulesToGrant([appPolicy({ blocked: true })], AT);
    expect(g).toEqual({
      v: 1,
      issuedAt: AT,
      rules: [{ pkg: "com.example.game", label: "Game", blocked: true }],
    });
  });

  it("carries a per-app schedule via scheduleToGrant", () => {
    const g = appRulesToGrant([appPolicy({ schedule: sched() })], AT);
    expect(g.rules[0].blocked).toBe(false);
    expect(g.rules[0].schedule).toEqual(scheduleToGrant(sched(), AT));
  });

  it("ignores device-scope policies and drops a blank appId", () => {
    const blank = appPolicy({ scope: { kind: "app", appId: "  ", label: "Z" } });
    const g = appRulesToGrant([devicePolicy(), appPolicy(), blank], AT);
    expect(g.rules.map((r) => r.pkg)).toEqual(["com.example.game"]);
  });
});

describe("lifelineToGrant / policyToClauses lifeline (spec D9)", () => {
  it("emits a lifeline clause with trimmed entries", () => {
    const p = devicePolicy({
      lifeline: { numbers: [{ label: " Mum ", number: " +44 7700 900123 " }] },
    });
    const clauses = policyToClauses(SUBJECT, p, AT);
    const lifeline = clauses.find((c) => c.kind === "lifeline");
    expect(lifeline).toBeDefined();
    expect(lifeline!.subject).toBe(SUBJECT);
    expect(lifeline!.issuedAt).toBe(AT);
    expect(lifeline!.body).toEqual({
      v: 1,
      numbers: [{ label: "Mum", number: "+44 7700 900123" }],
      issuedAt: AT,
    });
  });

  it("emits NO lifeline clause when absent or empty", () => {
    expect(
      policyToClauses(SUBJECT, devicePolicy(), AT).some((c) => c.kind === "lifeline"),
    ).toBe(false);
    expect(
      policyToClauses(SUBJECT, devicePolicy({ lifeline: { numbers: [] } }), AT).some(
        (c) => c.kind === "lifeline",
      ),
    ).toBe(false);
  });

  // v2 (2026-07-24): 5 numbers, the platform emergency entry, break-glass.
  it("keeps a v1 payload byte-identical when the v2 options are untouched", () => {
    const p = devicePolicy({
      lifeline: {
        numbers: [{ label: "Mum", number: "+44 7700 900123" }],
        emergencyServices: false,
      },
    });
    const body = policyToClauses(SUBJECT, p, AT).find((c) => c.kind === "lifeline")!.body;
    // Untouched means ABSENT on the wire — an older device must see exactly
    // what it saw before.
    expect(body).toEqual({
      v: 1,
      numbers: [{ label: "Mum", number: "+44 7700 900123" }],
      issuedAt: AT,
    });
  });

  it("says break-glass OFF out loud — the device reads an absent field as ON", () => {
    const p = devicePolicy({
      lifeline: {
        numbers: [{ label: "Mum", number: "+44 7700 900123" }],
        emergencyServices: false,
        breakGlass: { enabled: false, scope: "full", durationMinutes: 10 },
      },
    });
    const body = policyToClauses(SUBJECT, p, AT).find((c) => c.kind === "lifeline")!.body as {
      breakGlass?: { enabled: boolean; scope: string; durationMinutes: number };
    };
    // `BreakGlassCfg::safety_net` (charter-proto) makes absence mean enabled,
    // so an omitted "off" is a switch that does nothing on the phone.
    expect(body.breakGlass).toEqual({ enabled: false, scope: "full", durationMinutes: 10 });
  });

  it("carries five numbers, the emergency flag and break-glass when set", () => {
    const numbers = ["Mum", "Dad", "Nan", "Auntie", "Neighbour"].map((label, i) => ({
      label,
      number: `0770090012${i}`,
    }));
    const p = devicePolicy({
      lifeline: {
        numbers,
        emergencyServices: true,
        breakGlass: { enabled: true, scope: "calls", durationMinutes: 15 },
      },
    });
    const body = policyToClauses(SUBJECT, p, AT).find((c) => c.kind === "lifeline")!.body as {
      numbers: unknown[];
      emergencyServices?: boolean;
      breakGlass?: { enabled: boolean; scope: string; durationMinutes: number };
    };
    expect(body.numbers).toHaveLength(5);
    expect(body.emergencyServices).toBe(true);
    expect(body.breakGlass).toEqual({ enabled: true, scope: "calls", durationMinutes: 15 });
    // No emergency digits ever cross the wire — the device resolves them.
    expect(JSON.stringify(body)).not.toMatch(/\b(999|911|112)\b/);
  });
});

describe("appsToGrant — askFirst (named times' on-request apps)", () => {
  const apps = (over: Partial<AppsPolicy> = {}): AppsPolicy => ({
    enabled: true,
    posture: "blocklist",
    blocked: ["com.mojang.minecraftpe", "com.google.android.youtube"],
    allowed: [],
    ...over,
  });

  it("emits askFirst alongside blocked when every entry is genuinely blocked", () => {
    const g = appsToGrant(apps({ askFirst: ["com.mojang.minecraftpe"] }), 1700);
    expect(g.blocked).toEqual(["com.mojang.minecraftpe", "com.google.android.youtube"]);
    expect(g.askFirst).toEqual(["com.mojang.minecraftpe"]);
  });

  // The invariant the device relies on under BLOCKLIST posture: enforcement
  // reads `blocked` only and never consults `askFirst`, so a pkg advertised
  // as "ask to open" that isn't actually blocked would be a lie — the ask
  // button would do nothing. (The allowlist posture has its OWN invariant —
  // askFirst ∩ allowed = ∅ — covered below; "gated" means something
  // different under each posture, so one blanket "⊆ blocked" rule cannot
  // cover both.)
  it("INVARIANT (blocklist): every emitted askFirst pkg is also in blocked", () => {
    const g = appsToGrant(
      apps({ askFirst: ["com.mojang.minecraftpe", "org.not.blocked.at.all"] }),
      1700,
    );
    expect(g.askFirst).toEqual(["com.mojang.minecraftpe"]);
    for (const pkg of g.askFirst ?? []) {
      expect(g.blocked).toContain(pkg);
    }
  });

  it("drops askFirst entirely (never an empty array) when nothing survives the filter", () => {
    const g = appsToGrant(apps({ askFirst: ["org.not.blocked.at.all"] }), 1700);
    expect(g.askFirst).toBeUndefined();
    expect("askFirst" in g).toBe(false);
  });

  it("emits nothing extra when askFirst is unset (byte-identical to before)", () => {
    const g = appsToGrant(apps(), 1700);
    expect("askFirst" in g).toBe(false);
  });

  // F1: under allowlist, "blocked" is DERIVED (not-in-allowed), so the ward's
  // ask affordance can only be made real by emitting askFirst directly — a
  // pkg that isn't in `allowed` is already gated, with nothing further to
  // check against a (non-existent) `blocked` list.
  it("emits askFirst on an allowlist posture for a pkg NOT in allowed", () => {
    const g = appsToGrant(
      apps({ posture: "allowlist", allowed: ["org.mozilla.fenix"], askFirst: ["com.mojang.minecraftpe"] }),
      1700,
    );
    expect(g.blocked).toBeUndefined();
    expect(g.allowed).toEqual(["org.mozilla.fenix"]);
    expect(g.askFirst).toEqual(["com.mojang.minecraftpe"]);
  });

  // INVARIANT (allowlist): the mirror of the blocklist invariant above — an
  // askFirst pkg must never ALSO be in `allowed`, or the ask affordance would
  // be offered for an app that already runs everywhere unconditionally. N1
  // (review round 2): this is now guaranteed by `appsToGrant`'s OWN strip
  // (computed before the posture branch — `applyAppsFragment` no longer
  // strips anything, see its doc), so the invariant holds by construction —
  // this test proves it end-to-end rather than trusting an upstream filter.
  it("INVARIANT (allowlist): askFirst ∩ allowed = ∅ — the wire strips an already-allowed pkg out of allowed itself", () => {
    const g = appsToGrant(
      apps({
        posture: "allowlist",
        allowed: ["org.mozilla.fenix", "com.mojang.minecraftpe"],
        askFirst: ["com.mojang.minecraftpe", "org.other.app"],
      }),
      1700,
    );
    // The pkg that was (incorrectly, upstream — e.g. a stale manual allow
    // never reconciled with the new on-request group) ALSO in `allowed` is
    // stripped from the SIGNED `allowed` itself…
    expect(g.allowed).toEqual(["org.mozilla.fenix"]);
    // …and BOTH askFirst pkgs are still offered — an on-request pkg is never
    // silently dropped just because it used to be manually allowed; the
    // conflict resolves in favour of "ask first", not "always allowed".
    expect(g.askFirst).toEqual(["com.mojang.minecraftpe", "org.other.app"]);
    for (const pkg of g.askFirst ?? []) {
      expect(g.allowed).not.toContain(pkg);
    }
  });

  it("N1: an askFirst pkg that was the ONLY entry in allowed strips allowed to empty (undefined) and still offers the ask", () => {
    const g = appsToGrant(
      apps({
        posture: "allowlist",
        allowed: ["com.mojang.minecraftpe"],
        askFirst: ["com.mojang.minecraftpe"],
      }),
      1700,
    );
    expect(g.allowed).toBeUndefined();
    expect(g.askFirst).toEqual(["com.mojang.minecraftpe"]);
  });

  it("a paused (lifted) policy carries no askFirst, matching the standing lists", () => {
    const g = appsToGrant(apps({ enabled: false, askFirst: ["com.mojang.minecraftpe"] }), 1700);
    expect(g.paused).toBe(true);
    expect(g.askFirst).toBeUndefined();
  });
});

/**
 * "Remove from device" (2026-08-27): a ward tablet full of Samsung bloatware,
 * and no cable path left to strip it — ward 0.6.9 locks USB debugging by
 * design — so the warden's Device Owner power is the only route. The clause
 * axis is deliberately independent of everything the tests above cover.
 */
describe("appsToGrant — hidden (remove from device)", () => {
  const apps = (over: Partial<AppsPolicy> = {}): AppsPolicy => ({
    enabled: true,
    posture: "blocklist",
    blocked: ["com.google.android.youtube"],
    allowed: [],
    ...over,
  });

  it("emits hidden alongside the standing lists", () => {
    const g = appsToGrant(apps({ hidden: ["com.samsung.android.game.gamehome"] }), 1700);
    expect(g.hidden).toEqual(["com.samsung.android.game.gamehome"]);
    expect(g.blocked).toEqual(["com.google.android.youtube"]);
  });

  it("trims and dedupes, so a pasted package list can't emit the same app twice", () => {
    const g = appsToGrant(
      apps({
        hidden: ["  com.samsung.android.game.gamehome ", "com.samsung.android.game.gamehome", "com.sec.android.app.samsungapps"],
      }),
      1700,
    );
    expect(g.hidden).toEqual([
      "com.samsung.android.game.gamehome",
      "com.sec.android.app.samsungapps",
    ]);
  });

  it("drops hidden entirely (never an empty array) when nothing survives the trim", () => {
    const g = appsToGrant(apps({ hidden: ["", "   "] }), 1700);
    expect(g.hidden).toBeUndefined();
    expect("hidden" in g).toBe(false);
  });

  it("emits nothing extra when hidden is unset (byte-identical to before)", () => {
    const g = appsToGrant(apps(), 1700);
    expect("hidden" in g).toBe(false);
  });

  // The whole point of the axis: `paused` is app CONTROL's off switch. A
  // guardian lifting app blocks for the holidays has not asked for forty
  // preinstalled apps back on their child's home screen, so the removal
  // survives the lift — and the clause still says `paused: true` for
  // everything else.
  it("emits hidden even on a paused (enabled:false) policy", () => {
    const g = appsToGrant(
      apps({ enabled: false, hidden: ["com.samsung.android.game.gamehome"] }),
      1700,
    );
    expect(g.paused).toBe(true);
    expect(g.hidden).toEqual(["com.samsung.android.game.gamehome"]);
    // …and nothing from the standing lists rides along with it.
    expect(g.blocked).toBeUndefined();
    expect(g.allowed).toBeUndefined();
  });

  it("is independent of posture — an allowlist family removes apps the same way", () => {
    const g = appsToGrant(
      apps({
        posture: "allowlist",
        allowed: ["org.mozilla.fenix"],
        hidden: ["com.sec.android.app.samsungapps"],
      }),
      1700,
    );
    expect(g.allowed).toEqual(["org.mozilla.fenix"]);
    expect(g.hidden).toEqual(["com.sec.android.app.samsungapps"]);
  });
});
