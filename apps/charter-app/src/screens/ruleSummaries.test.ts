import { describe, expect, it } from "vitest";
import { ruleSummaries } from "./ruleSummaries";
import type { Policy } from "../domain/types";

const NOW = Math.floor(Date.parse("2026-10-29T12:00:00Z") / 1000);

describe("ruleSummaries (the child page's rule rows)", () => {
  it("says what an untouched device policy currently means, in the Limits accordion's words", () => {
    const s = ruleSummaries({ id: "p", scope: { kind: "device" } }, NOW);
    expect(s.schedule).toBe("No schedule set yet");
    expect(s.budget).toBe("No daily limit");
    expect(s["named-times"]).toBe("No named times yet");
    expect(s.listening).toBe("Audio stops with the screen");
    expect(s["always-available"]).toBe("Nothing — the lock takes everything");
    expect(s.web).toBe("Web filtering off — all sites allowed");
    expect(s.apps).toBe("No app blocks");
    expect(s.tethering).toBe("Hotspot off");
    // Absent means ON, matching the device.
    expect(s.lifeline).toContain("emergency unlock");
  });

  it("reads the saved schedule, limit and apps", () => {
    const policy: Policy = {
      id: "p",
      scope: { kind: "device" },
      schedule: {
        tz: "UTC",
        weekly: {
          mon: [{ start: "16:00", end: "18:00" }],
          tue: [{ start: "16:00", end: "18:00" }],
        },
      },
      budget: { tz: "UTC", dailyMinutes: 90 },
      apps: { enabled: true, posture: "blocklist", blocked: ["a", "b"], allowed: [] },
    };
    const s = ruleSummaries(policy, NOW);
    expect(s.schedule).toBe("Allowed 2 days a week");
    expect(s.budget).toBe("1h 30m a day");
    expect(s.apps).toBe("2 apps blocked");
  });
});
