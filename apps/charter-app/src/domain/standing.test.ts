import { describe, expect, it } from "vitest";
import {
  computeStanding,
  deviceSetLimits,
  shortDuration,
  standingFor,
  standingNote,
} from "./standing";

describe("computeStanding", () => {
  it("reports the overdraft when a ward is past the daily cap", () => {
    // Rob's real numbers on 2026-07-31: 120-minute cap, 11118s used.
    const s = computeStanding({ usedTodaySecs: 11118, dailyMinutes: 120 });
    expect(s.overdraftTodaySecs).toBe(11118 - 7200);
  });

  it("reports no overdraft while still inside the cap", () => {
    expect(computeStanding({ usedTodaySecs: 3600, dailyMinutes: 120 }).overdraftTodaySecs).toBe(0);
  });

  it("has no opinion when there is no daily cap", () => {
    expect(computeStanding({ usedTodaySecs: 99999 }).overdraftTodaySecs).toBe(0);
  });

  it("reports what is left in the week", () => {
    const s = computeStanding({ usedWeekSecs: 29781, weeklyMinutes: 600 });
    expect(s.weekLeftSecs).toBe(600 * 60 - 29781);
  });

  it("floors an overdrawn week at zero rather than going negative", () => {
    expect(computeStanding({ usedWeekSecs: 99999, weeklyMinutes: 60 }).weekLeftSecs).toBe(0);
  });

  it("says nothing about the week when the reading is missing", () => {
    // Silence beats implying the pot is full — an unknown is not "fine".
    expect(computeStanding({ weeklyMinutes: 600 }).weekLeftSecs).toBeNull();
    expect(computeStanding({ usedWeekSecs: 100 }).weekLeftSecs).toBeNull();
  });
});

describe("standingNote", () => {
  it("stays silent for a ward inside their limits", () => {
    // Crying wolf is how warnings stop being read.
    expect(standingNote({ overdraftTodaySecs: 0, weekLeftSecs: 5 * 3600 })).toBeNull();
  });

  it("names the overdraft", () => {
    expect(standingNote({ overdraftTodaySecs: 3918, weekLeftSecs: null })).toBe("1h 5m over today");
  });

  it("warns only when the week is genuinely tight", () => {
    expect(standingNote({ overdraftTodaySecs: 0, weekLeftSecs: 20 * 60 })).toBe(
      "20m left this week",
    );
    expect(standingNote({ overdraftTodaySecs: 0, weekLeftSecs: 3 * 3600 })).toBeNull();
  });

  it("says both when both are true", () => {
    expect(standingNote({ overdraftTodaySecs: 600, weekLeftSecs: 0 })).toBe(
      "10m over today · nothing left this week",
    );
  });
});

describe("shortDuration", () => {
  it("reads the way a person would say it", () => {
    expect(shortDuration(45)).toBe("45s");
    expect(shortDuration(1200)).toBe("20m");
    expect(shortDuration(3600)).toBe("1h");
    expect(shortDuration(3918)).toBe("1h 5m");
    expect(shortDuration(7500)).toBe("2h 5m");
  });
});

describe("standingFor", () => {
  const budgetPolicy = (dailyMinutes: number | null, weeklyMinutes?: number | null) => ({
    scope: { kind: "device" },
    budget: { dailyMinutes, weeklyMinutes },
  });

  it("reads the ward's own limits against their device's reading", () => {
    const s = standingFor(
      { policies: [budgetPolicy(120)], devices: [{ devicePubkey: "a" }] },
      { a: { usedTodaySecs: 11118 } },
    );
    expect(s.overdraftTodaySecs).toBe(3918);
  });

  it("takes the highest reading, never the sum", () => {
    // Each warden reports the POOLED figure it enforces against; summing would
    // double-count a ward with a laptop and a phone and invent an overdraft.
    const s = standingFor(
      {
        policies: [budgetPolicy(60)],
        devices: [{ devicePubkey: "a" }, { devicePubkey: "b" }],
      },
      { a: { usedTodaySecs: 3600 }, b: { usedTodaySecs: 3600 } },
    );
    expect(s.overdraftTodaySecs).toBe(0);
  });

  it("has no opinion while a budget is paused", () => {
    const s = standingFor(
      {
        policies: [{ scope: { kind: "device" }, budget: { dailyMinutes: 60, paused: true } }],
        devices: [{ devicePubkey: "a" }],
      },
      { a: { usedTodaySecs: 99999 } },
    );
    expect(s.overdraftTodaySecs).toBe(0);
  });

  it("has no opinion when no device has reported", () => {
    const s = standingFor(
      { policies: [budgetPolicy(60)], devices: [{ devicePubkey: "a" }] },
      {},
    );
    expect(s.overdraftTodaySecs).toBe(0);
  });
});

describe("limits set on the device rather than from the phone", () => {
  const noPolicy = { policies: [], devices: [{ devicePubkey: "a" }] };

  it("measures the overdraft against the DEVICE's limit when the app has none", () => {
    // Rob's real case: 2h/day set by charter-setup on the laptop, never from
    // Kintrinsic. Without this the app has no cap and silently reports "not
    // over" — which is not the same as "within limits".
    const s = standingFor(noPolicy, { a: { usedTodaySecs: 14100, dailyMinutes: 120 } });
    expect(s.overdraftTodaySecs).toBe(14100 - 7200); // 1h 55m over
  });

  it("prefers the guardian's own limit when they have set one", () => {
    const s = standingFor(
      {
        policies: [{ scope: { kind: "device" }, budget: { dailyMinutes: 60 } }],
        devices: [{ devicePubkey: "a" }],
      },
      { a: { usedTodaySecs: 7200, dailyMinutes: 120 } },
    );
    expect(s.overdraftTodaySecs).toBe(3600); // measured against 60, not 120
  });

  it("takes the tightest cap when two devices report different ones", () => {
    const s = standingFor(
      { policies: [], devices: [{ devicePubkey: "a" }, { devicePubkey: "b" }] },
      { a: { dailyMinutes: 120, usedTodaySecs: 5400 }, b: { dailyMinutes: 60 } },
    );
    expect(s.overdraftTodaySecs).toBe(5400 - 3600);
  });

  it("reports device-set limits so the guardian can be warned before replacing them", () => {
    expect(
      deviceSetLimits(noPolicy, { a: { dailyMinutes: 120, source: "device-only" } }),
    ).toEqual({ dailyMinutes: 120, weeklyMinutes: undefined });
  });

  it("stays quiet once the guardian's own limits are in force", () => {
    expect(
      deviceSetLimits(
        {
          policies: [{ scope: { kind: "device" }, budget: { dailyMinutes: 90 } }],
          devices: [{ devicePubkey: "a" }],
        },
        { a: { dailyMinutes: 120, source: "device-only" } },
      ),
    ).toBeNull();
  });

  it("ignores a device already governed by the guardian", () => {
    expect(
      deviceSetLimits(noPolicy, { a: { dailyMinutes: 120, source: "guardian" } }),
    ).toBeNull();
  });
});
