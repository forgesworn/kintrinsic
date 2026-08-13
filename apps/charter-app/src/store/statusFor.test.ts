import { describe, expect, it } from "vitest";
import { statusFor, formatClock } from "./statusFor";
import type { Child } from "../domain/types";

function childWithWindow(): Child {
  return {
    id: "c1",
    name: "Sam",
    color: "#000",
    dependantPubkey: null,
    devices: [
      {
        id: "dev1",
        label: "Sam's laptop",
        platform: "linux",
        pairing: "paired",
      },
    ],
    policies: [
      {
        id: "p1",
        scope: { kind: "device" },
        schedule: {
          tz: "UTC",
          weekly: {
            mon: [{ start: "16:00", end: "18:00" }],
            tue: [{ start: "16:00", end: "18:00" }],
            wed: [{ start: "16:00", end: "18:00" }],
            thu: [{ start: "16:00", end: "18:00" }],
            fri: [{ start: "16:00", end: "18:00" }],
            sat: [{ start: "16:00", end: "18:00" }],
            sun: [{ start: "16:00", end: "18:00" }],
          },
        },
        budget: { tz: "UTC", dailyMinutes: 90 },
      },
    ],
  };
}

// Build a local Date at a given hour:minute today.
function at(hour: number, minute = 0): number {
  const d = new Date();
  d.setHours(hour, minute, 0, 0);
  return d.getTime();
}

describe("statusFor", () => {
  it("blocks outside the window and offers next-open text", () => {
    const r = statusFor(childWithWindow(), at(9, 0));
    expect(r.allowedNow).toBe(false);
    expect(r.reason).toBe("schedule");
    expect(r.minutesLeftToday).toBe(0);
    expect(r.nextWindowText).toBe("Opens at 4:00 PM");
  });

  it("allows inside the window, capped by min(window, budget)", () => {
    // 17:00 -> 60 min of window left, budget 90 -> effective 60.
    const r = statusFor(childWithWindow(), at(17, 0));
    expect(r.allowedNow).toBe(true);
    expect(r.reason).toBeUndefined();
    expect(r.minutesLeftToday).toBe(60);
  });

  it("returns unlimited when there is no device policy", () => {
    const child = { ...childWithWindow(), policies: [] };
    const r = statusFor(child, at(9));
    expect(r.allowedNow).toBe(true);
    expect(r.minutesLeftToday).toBeNull();
  });
});

describe("formatClock", () => {
  it("renders 24h as friendly 12h", () => {
    expect(formatClock("16:00")).toBe("4:00 PM");
    expect(formatClock("00:30")).toBe("12:30 AM");
  });
});
