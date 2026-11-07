import { describe, expect, it } from "vitest";
import type { Policy } from "../domain/types";
import { policyToClauses, appRulesToGrant, debuggingToGrant } from "./clause";
import { readClause } from "./clauseReverse";

const T = 1_800_000_000;

const full: Policy = {
  id: "p",
  scope: { kind: "device" },
  times: [],
  schedule: { tz: "Europe/London", weekly: { mon: [{ start: "08:00", end: "18:00" }] }, overrides: { "2026-12-25": [{ start: "09:00", end: "20:00" }] } },
  budget: { tz: "Europe/London", dailyMinutes: 90, weekStart: "mon" },
  web: { enabled: true, posture: "blocklist", ageTier: "older", allow: ["a.org"], block: ["b.com"], youtube: "strict", safeSearch: true },
  apps: { enabled: true, posture: "allowlist", blocked: [], allowed: ["x", "y"], askFirst: ["y"], hidden: ["z"] },
  learning: { enabled: true, apps: [{ id: "site-a", label: "A", kind: "site", domains: ["a.org"], url: "https://a.org", free: false }], capMinutes: 30 },
  tethering: { allow: "filtered", until: T + 100 },
  connections: { wifi: "saved", bluetooth: "paired" },
  lifeline: { numbers: [{ label: "Mum", number: "0700" }], torch: true, breakGlass: { enabled: false, scope: "calls", durationMinutes: 10 } },
  buckets: { enabled: true, tz: "Europe/London", buckets: [{ id: "games", label: "Games", apps: ["g"], weeklyMinutes: 300 }] },
  listening: { mode: "grace", graceMinutes: 20, apps: ["music"] },
  alwaysAvailable: { apps: [{ pkg: "maps" }, { pkg: "notes", untilUnix: T + 60 }] },
};

describe("reading a received clause back onto the screens (decision 3)", () => {
  it("reads every kind this app signs back to the same rule", () => {
    for (const c of policyToClauses(null, full, T)) {
      const read = readClause(c);
      expect(read, c.kind).not.toBeNull();
    }
    expect(readClause(policyToClauses(null, full, T).find((c) => c.kind === "schedule"))!.value).toEqual(full.schedule);
    expect(readClause(policyToClauses(null, full, T).find((c) => c.kind === "apps"))!.value).toEqual(full.apps);
  });

  it("reads the lifted forms back as lifted", () => {
    const lifted: Policy = {
      id: "p",
      scope: { kind: "device" },
      schedule: { tz: "UTC", weekly: {}, paused: true },
      budget: { tz: "UTC", paused: true },
      web: { ...full.web!, enabled: false },
      apps: { ...full.apps!, enabled: false, holds: undefined },
      learning: { enabled: false, apps: full.learning!.apps },
    };
    const byKind = Object.fromEntries(policyToClauses(null, lifted, T).map((c) => [c.kind, readClause(c, { policy: lifted })]));
    expect(byKind.schedule!.value).toEqual({ tz: "UTC", weekly: {}, paused: true });
    expect(byKind.budget!.value).toEqual({ tz: "UTC", paused: true });
    // What the wire doesn't carry stays as this phone had it.
    expect(byKind.content!.value).toEqual(lifted.web);
    expect((byKind.apps!.value as { allowed: string[] }).allowed).toEqual(["x", "y"]);
    // "Block all time" (wire paused) is a schedule with no windows.
    const blockAll = policyToClauses(null, { id: "p", scope: { kind: "device" }, schedule: { tz: "UTC", weekly: {} } }, T)[0];
    expect(readClause(blockAll)!.value).toEqual({ tz: "UTC", weekly: {} });
  });

  it("reads singled-out apps and USB debugging", () => {
    const body = appRulesToGrant(
      [{ id: "a", scope: { kind: "app", appId: "game", label: "Game" }, blocked: false, schedule: full.schedule }],
      T,
    );
    expect(readClause({ v: 1, kind: "apprules", issuedAt: T, body })!.value).toEqual([
      { appId: "game", label: "Game", blocked: false, schedule: full.schedule },
    ]);
    expect(readClause({ v: 1, kind: "debugging", issuedAt: T, body: debuggingToGrant({ allow: true, until: T + 5 }, T) })!.value).toEqual({
      allow: true,
      until: T + 5,
    });
  });

  it("calls a clause lossy when the screen can't hold it as it is", () => {
    // A revoked budget, a curated content list, an unknown field: none round-trip.
    expect(readClause({ v: 1, kind: "budget", issuedAt: T, body: { v: 1, tz: "UTC", revoked: true, issuedAt: T } })).toBeNull();
    expect(
      readClause({ v: 1, kind: "content", issuedAt: T, body: { v: 1, posture: "blocklist", ageTier: "older", curators: ["c"], issuedAt: T } }),
    ).toBeNull();
    expect(
      readClause({ v: 1, kind: "connections", issuedAt: T, body: { v: 1, issuedAt: T, wifi: "any", bluetooth: "any", nfc: "off" } }),
    ).toBeNull();
    // Blocking all time while listing windows: the app never sends it.
    expect(
      readClause({ v: 1, kind: "schedule", issuedAt: T, body: { v: 1, tz: "UTC", paused: true, weekly: { mon: [{ start: "08:00", end: "09:00" }] }, issuedAt: T } }),
    ).toBeNull();
    // No screen edits these.
    expect(readClause({ v: 1, kind: "maintenance", issuedAt: T, body: { v: 1, untilUnix: T, issuedAt: T } })).toBeNull();
    expect(readClause({ v: 1, kind: "schedule", issuedAt: T, body: null })).toBeNull();
  });
});
