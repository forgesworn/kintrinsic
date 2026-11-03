import { describe, expect, it } from "vitest";
import { overrideSummary } from "./overrideSummary";

describe("overrideSummary", () => {
  it("says what was opened and for how long, like the carrier's notification", () => {
    expect(overrideSummary("Pixel", "full", 300)).toBe("Opened their phone on Pixel for 5 minutes");
    expect(overrideSummary("Pixel", "calls", 60)).toBe("Opened calls on Pixel for 1 minute");
  });

  it("rounds a short window down to whole minutes but never below one", () => {
    expect(overrideSummary("Pixel", "full", 30)).toBe("Opened their phone on Pixel for 1 minute");
    expect(overrideSummary("Pixel", "full", 179)).toBe("Opened their phone on Pixel for 2 minutes");
  });

  it("leaves out what the ward's phone did not report", () => {
    expect(overrideSummary("Pixel", null, 300)).toBe("Used on Pixel for 5 minutes");
    expect(overrideSummary("Pixel", "full", null)).toBe("Opened their phone on Pixel");
    expect(overrideSummary("Pixel", null, null)).toBe("Used on Pixel");
  });
});
