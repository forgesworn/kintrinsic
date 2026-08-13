import { describe, expect, it } from "vitest";
import { bucketsToGrant, policyToClauses } from "./clause";
import type { BucketsPolicy, Policy } from "../domain/types";

const play = (over: Partial<BucketsPolicy> = {}): BucketsPolicy => ({
  enabled: true,
  tz: "Europe/London",
  buckets: [
    {
      id: "play",
      label: "Play",
      apps: ["com.mojang.Minecraft", "/usr/games/supertux2"],
      dailyMinutes: 60,
    },
  ],
  ...over,
});

describe("bucketsToGrant", () => {
  it("carries the bucket to the wire in the contract's shape", () => {
    const g = bucketsToGrant(play(), 1700);
    expect(g).toEqual({
      v: 1,
      tz: "Europe/London",
      issuedAt: 1700,
      buckets: [
        {
          id: "play",
          label: "Play",
          apps: ["com.mojang.Minecraft", "/usr/games/supertux2"],
          dailyMinutes: 60,
        },
      ],
    });
  });

  it("turning buckets off pauses rather than deletes them", () => {
    const g = bucketsToGrant(play({ enabled: false }), 1700);
    expect(g.paused).toBe(true);
    // The set survives, so switching back on restores the family's setup.
    expect(g.buckets).toHaveLength(1);
  });

  /**
   * The device validates fail-safe: a malformed body caps NOTHING. So a
   * half-filled bucket must never reach the wire, or it would silently
   * disable every OTHER bucket in the same clause.
   */
  it("drops incomplete buckets instead of shipping them", () => {
    const g = bucketsToGrant(
      play({
        buckets: [
          { id: "play", label: "Play", apps: ["mc"], dailyMinutes: 60 },
          { id: "empty", label: "No apps yet", apps: [], dailyMinutes: 60 },
          { id: "nolabel", label: "   ", apps: ["x"], dailyMinutes: 60 },
          { id: "Bad Id", label: "Bad", apps: ["x"], dailyMinutes: 60 },
          { id: "zero", label: "Zero", apps: ["x"], dailyMinutes: 0 },
          { id: "huge", label: "Huge", apps: ["x"], dailyMinutes: 5000 },
        ],
      }),
      1700,
    );
    expect(g.buckets.map((b) => b.id)).toEqual(["play"]);
  });

  it("trims whitespace and drops blank app identities", () => {
    const g = bucketsToGrant(
      play({ buckets: [{ id: "play", label: " Play ", apps: [" mc ", "  "], dailyMinutes: 60 }] }),
      1700,
    );
    expect(g.buckets[0]).toEqual({
      id: "play",
      label: "Play",
      apps: ["mc"],
      dailyMinutes: 60,
    });
  });
});

/**
 * THE VERSION RULE (binding, byte-pinned both directions — see the Rust
 * `wire_agreement` twin in `charter-schedule::buckets`): v1 iff EVERY bucket
 * has dailyMinutes; v2 the moment ANY bucket is weekly-only.
 */
describe("bucketsToGrant — the v1/v2 version rule", () => {
  it("stays v1 when every bucket carries dailyMinutes (byte-identical to pre-weekly)", () => {
    const g = bucketsToGrant(play(), 1700);
    expect(g.v).toBe(1);
    expect(g.buckets[0].weeklyMinutes).toBeUndefined();
  });

  it("emits v2 the moment ANY bucket is weekly-only", () => {
    const g = bucketsToGrant(
      play({
        buckets: [
          { id: "play", label: "Play", apps: ["mc"], dailyMinutes: 60 },
          { id: "social", label: "Social", apps: ["chat"], weeklyMinutes: 300 },
        ],
      }),
      1700,
    );
    expect(g.v).toBe(2);
  });

  it("a set with both daily+weekly on the same bucket is still v1 (dailyMinutes present)", () => {
    const g = bucketsToGrant(
      play({
        buckets: [
          { id: "play", label: "Play", apps: ["mc"], dailyMinutes: 60, weeklyMinutes: 300 },
        ],
      }),
      1700,
    );
    expect(g.v).toBe(1);
    expect(g.buckets[0]).toEqual({
      id: "play",
      label: "Play",
      apps: ["mc"],
      dailyMinutes: 60,
      weeklyMinutes: 300,
    });
  });

  it("an empty bucket set stays v1 (vacuously — nothing to disagree with a pre-weekly ward about)", () => {
    const g = bucketsToGrant(play({ buckets: [] }), 1700);
    expect(g.v).toBe(1);
  });

  it("carries weekStart when set, and omits it when not (byte-identical to before)", () => {
    const withWeek = bucketsToGrant(play({ weekStart: "sun" }), 1700);
    expect(withWeek.weekStart).toBe("sun");
    const without = bucketsToGrant(play(), 1700);
    expect(without.weekStart).toBeUndefined();
    expect("weekStart" in without).toBe(false);
  });

  it("contract.md's weekly-only worked example round-trips exactly", () => {
    const g = bucketsToGrant(
      {
        enabled: true,
        tz: "Europe/London",
        weekStart: "mon",
        buckets: [
          { id: "play", label: "Play", apps: ["com.mojang.minecraftpe"], weeklyMinutes: 300 },
        ],
      },
      1732550400,
    );
    expect(g).toEqual({
      v: 2,
      buckets: [
        { id: "play", label: "Play", apps: ["com.mojang.minecraftpe"], weeklyMinutes: 300 },
      ],
      tz: "Europe/London",
      issuedAt: 1732550400,
      weekStart: "mon",
    });
    // The exact bytes the Rust `wire_agreement` test pins.
    expect(JSON.stringify(g)).toBe(
      '{"v":2,"buckets":[{"id":"play","label":"Play","apps":["com.mojang.minecraftpe"],"weeklyMinutes":300}],"tz":"Europe/London","issuedAt":1732550400,"weekStart":"mon"}',
    );
  });
});

describe("bucketsToGrant — weekly-only validation bounds", () => {
  it("drops a bucket with neither dailyMinutes nor weeklyMinutes", () => {
    const g = bucketsToGrant(
      play({ buckets: [{ id: "empty", label: "Empty", apps: ["x"] }] }),
      1700,
    );
    expect(g.buckets).toHaveLength(0);
  });

  it("keeps a valid weekly-only bucket and rejects out-of-range weeklyMinutes", () => {
    const inBounds = bucketsToGrant(
      play({ buckets: [{ id: "play", label: "Play", apps: ["x"], weeklyMinutes: 10080 }] }),
      1700,
    );
    expect(inBounds.buckets.map((b) => b.id)).toEqual(["play"]);

    const zero = bucketsToGrant(
      play({ buckets: [{ id: "play", label: "Play", apps: ["x"], weeklyMinutes: 0 }] }),
      1700,
    );
    expect(zero.buckets).toHaveLength(0);

    const tooBig = bucketsToGrant(
      play({ buckets: [{ id: "play", label: "Play", apps: ["x"], weeklyMinutes: 10081 }] }),
      1700,
    );
    expect(tooBig.buckets).toHaveLength(0);
  });

  it("a bucket with both axes set must satisfy both bounds independently", () => {
    const g = bucketsToGrant(
      play({
        buckets: [
          { id: "ok", label: "Ok", apps: ["x"], dailyMinutes: 60, weeklyMinutes: 300 },
          { id: "bad-weekly", label: "Bad", apps: ["x"], dailyMinutes: 60, weeklyMinutes: 99999 },
        ],
      }),
      1700,
    );
    expect(g.buckets.map((b) => b.id)).toEqual(["ok"]);
  });
});

describe("policyToClauses", () => {
  it("emits a buckets clause alongside the others", () => {
    const policy: Policy = {
      id: "p1",
      scope: { kind: "device" },
      buckets: play(),
    };
    const clauses = policyToClauses(null, policy, 1700);
    const buckets = clauses.find((c) => c.kind === "buckets");
    expect(buckets).toBeDefined();
    expect(buckets?.body).toEqual(bucketsToGrant(play(), 1700));
  });

  /** A child with no buckets must emit byte-identically to before this existed. */
  it("emits nothing when the family never set one up", () => {
    const policy: Policy = { id: "p1", scope: { kind: "device" } };
    expect(policyToClauses(null, policy, 1700).some((c) => c.kind === "buckets")).toBe(false);
  });
});
