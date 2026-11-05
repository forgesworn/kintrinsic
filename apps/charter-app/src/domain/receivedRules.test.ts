import { describe, expect, it } from "vitest";
import { seedChild, setElsewhere, type HeldRule, type InForceLookup } from "./receivedRules";
import type { Child, Policy } from "./types";
import { appRulesToGrant, debuggingToGrant, scheduleToGrant, budgetToGrant } from "../wire/clause";

const T = 1_800_000_000;
const PHONE = "11".repeat(32);
const LAPTOP = "22".repeat(32);
const OTHER = "33".repeat(32);

let n = 0;
const rule = (kind: string, body: unknown, issuedAt = T): HeldRule => ({
  id: (++n).toString(16).padStart(64, "0"),
  pubkey: OTHER,
  content: JSON.stringify({ v: 1, kind, issuedAt, body }),
});

const tz = "Europe/London";
const base: Policy = {
  id: "pol",
  scope: { kind: "device" },
  schedule: { tz, weekly: { mon: [{ start: "07:00", end: "08:00" }] } },
  budget: { tz, dailyMinutes: 60 },
};

function child(policies: Policy[] = [base]): Child {
  return {
    id: "c1",
    name: "Sam",
    color: "#000",
    dependantPubkey: null,
    rosterId: "aa".repeat(32),
    policies,
    devices: [
      { id: "d-phone", label: "Phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
      { id: "d-laptop", label: "Laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
    ] as Child["devices"],
  };
}

const mint = () => "pol-new";

describe("seeding received rules onto this phone's screens (decision 3)", () => {
  it("shows a lossless family-wide rule as this phone's own, and drops the per-device copies it replaced", () => {
    const sched = { tz, weekly: { tue: [{ start: "09:00", end: "10:00" }] } };
    const split: Policy = { ...base, deviceOverrides: { "d-phone": { schedule: { tz, weekly: {} } }, "d-laptop": { budget: { tz, dailyMinutes: 5 } } } };
    const r = rule("schedule", scheduleToGrant(sched, T));
    const out = seedChild(child([split]), { schedule: r }, { schedule: r }, mint);
    expect(out.applied).toEqual({ schedule: r.id });
    const p = out.policies[0];
    expect(p.schedule).toEqual(sched);
    expect(p.deviceOverrides).toEqual({ "d-laptop": { budget: { tz, dailyMinutes: 5 } } });
    expect(p.budget).toEqual(base.budget);
  });

  it("puts a device rule on that device only, and waits for a device this phone doesn't know", () => {
    const r = rule("budget", budgetToGrant({ tz, dailyMinutes: 30 }, T));
    const unknown = rule("budget", budgetToGrant({ tz, dailyMinutes: 10 }, T));
    const held = { [`budget@${PHONE}`]: r, [`budget@${"44".repeat(32)}`]: unknown };
    const out = seedChild(child(), held, held, mint);
    expect(out.applied).toEqual({ [`budget@${PHONE}`]: r.id });
    expect(out.policies[0].deviceOverrides).toEqual({ "d-phone": { budget: { tz, dailyMinutes: 30 } } });
    expect(out.policies[0].budget).toEqual(base.budget);
  });

  it("leaves a lossy rule off the screens but marks it dealt with", () => {
    const r = rule("budget", { v: 1, tz, revoked: true, issuedAt: T });
    const c = child();
    const out = seedChild(c, { budget: r }, { budget: r }, mint);
    expect(out.applied).toEqual({ budget: r.id });
    expect(out.policies).toBe(c.policies);
  });

  it("replaces the singled-out apps as a set, keeping ids it knows, and seeds USB debugging", () => {
    const game: Policy = { id: "pol-game", scope: { kind: "app", appId: "game", label: "Game" }, blocked: true };
    const old: Policy = { id: "pol-old", scope: { kind: "app", appId: "old", label: "Old" }, blocked: true };
    const ar = rule(
      "apprules",
      appRulesToGrant(
        [
          { id: "x", scope: { kind: "app", appId: "game", label: "Game" }, blocked: false },
          { id: "y", scope: { kind: "app", appId: "chat", label: "Chat" }, blocked: true },
        ],
        T,
      ),
    );
    const dbg = rule("debugging", debuggingToGrant({ allow: true, until: T + 60 }, T));
    const held = { apprules: ar, [`debugging@${LAPTOP}`]: dbg };
    const out = seedChild(child([base, game, old]), held, held, mint);
    expect(out.policies.map((p) => [p.id, p.scope.kind === "app" ? p.scope.appId : "-", p.blocked])).toEqual([
      ["pol", "-", undefined],
      ["pol-game", "game", false],
      ["pol-new", "chat", true],
    ]);
    expect(out.debugging).toEqual([{ deviceId: "d-laptop", debugging: { allow: true, until: T + 60, issuedAt: T } }]);
  });

  it("names a lossy rule in force as set elsewhere; a local or lossless one stays this phone's", () => {
    const lossy = rule("budget", { v: 1, tz, revoked: true, issuedAt: T });
    const fine = rule("schedule", scheduleToGrant(base.schedule!, T));
    const lookup = (local: boolean): InForceLookup => (kind) =>
      kind === "budget" ? { event: lossy, local } : kind === "schedule" ? { event: fine, local: false } : undefined;
    expect(setElsewhere(child(), ["schedule", "budget"], lookup(false))).toEqual([{ kind: "budget", author: OTHER, issuedAt: T }]);
    expect(setElsewhere(child(), ["schedule", "budget"], lookup(true))).toEqual([]);
  });
});
