import { describe, expect, it } from "vitest";
import { resolveChildTz } from "./childTz";
import type { Policy } from "./types";

function policy(over: Partial<Policy> = {}): Policy {
  return {
    id: "p1",
    scope: { kind: "device" },
    ...over,
  };
}

/**
 * C-1 (hardware round, 2026-08-03): a buckets-only ward — a counted/on-request
 * group and nothing else, the feature's own minimal configuration — carries
 * neither `schedule.tz` nor `budget.tz`. Every guardian-side tz lookup that
 * stopped at those two left such a ward's gift/stand-down/app.open "Rest of
 * today" signing unreachable. `resolveChildTz` is the one place all of them
 * now go through.
 */
describe("resolveChildTz", () => {
  it("prefers schedule.tz", () => {
    const p = policy({
      schedule: { tz: "Asia/Tokyo", weekly: {} },
      budget: { tz: "America/Chicago" },
      buckets: { enabled: true, tz: "Europe/London", buckets: [] },
    });
    expect(resolveChildTz(p)).toBe("Asia/Tokyo");
  });

  it("falls back to budget.tz when schedule is absent", () => {
    const p = policy({
      budget: { tz: "America/Chicago" },
      buckets: { enabled: true, tz: "Europe/London", buckets: [] },
    });
    expect(resolveChildTz(p)).toBe("America/Chicago");
  });

  it("falls all the way through to the buckets clause's own tz — the buckets-only ward (C-1)", () => {
    const p = policy({
      buckets: { enabled: true, tz: "Europe/London", buckets: [] },
    });
    expect(resolveChildTz(p)).toBe("Europe/London");
  });

  it("is undefined for a policy with none of the three, and for an absent policy", () => {
    expect(resolveChildTz(policy())).toBeUndefined();
    expect(resolveChildTz(undefined)).toBeUndefined();
  });
});
