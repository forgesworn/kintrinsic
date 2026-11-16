import { describe, expect, it } from "vitest";
import {
  emergencyCallSummary,
  emergencyControlSummary,
  overrideSummary,
  type ReleasedNote,
} from "./overrideSummary";

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

describe("the three forms for a phone the guardian has disconnected", () => {
  const before: ReleasedNote = { childName: "Mia", order: "before" };
  const after: ReleasedNote = { childName: "Mia", order: "after" };
  const unknown: ReleasedNote = { childName: "Mia", order: "unknown" };
  const NOT_RECEIVED = "The phone may not have received the disconnect.";

  it("the emergency call button: before, after, and no order claimed", () => {
    expect(emergencyCallSummary("Pixel", before)).toBe(
      "Used the emergency call button on Pixel, before you disconnected it from Mia. Kintrinsic can't see whether a call was made.",
    );
    expect(emergencyCallSummary("Pixel", after)).toBe(
      `Used the emergency call button on Pixel, after you disconnected it from Mia. ${NOT_RECEIVED} Kintrinsic can't see whether a call was made.`,
    );
    expect(emergencyCallSummary("Pixel", unknown)).toBe(
      "Used the emergency call button on Pixel, a phone you have disconnected. Kintrinsic can't see whether a call was made.",
    );
  });

  it("the emergency unlock: before, after, and no order claimed", () => {
    expect(overrideSummary("Pixel", "full", 300, before)).toBe(
      "Opened their phone on Pixel for 5 minutes, before you disconnected it from Mia",
    );
    expect(overrideSummary("Pixel", "full", 300, after)).toBe(
      `Opened their phone on Pixel for 5 minutes, after you disconnected it from Mia. ${NOT_RECEIVED}`,
    );
    expect(overrideSummary("Pixel", "full", 300, unknown)).toBe(
      "Opened their phone on Pixel for 5 minutes, a phone you have disconnected",
    );
  });

  it("an emergency control this build does not know: before, after, and no order claimed", () => {
    expect(emergencyControlSummary("Pixel", before)).toBe(
      "An emergency control was used on Pixel, before you disconnected it from Mia.",
    );
    expect(emergencyControlSummary("Pixel", after)).toBe(
      `An emergency control was used on Pixel, after you disconnected it from Mia. ${NOT_RECEIVED}`,
    );
    expect(emergencyControlSummary("Pixel", unknown)).toBe(
      "An emergency control was used on Pixel, a phone you have disconnected.",
    );
  });

  it("a connected device gets no disconnect wording at all", () => {
    expect(emergencyControlSummary("Pixel")).toBe("An emergency control was used on Pixel.");
    expect(emergencyCallSummary("Pixel")).not.toContain("disconnect");
    expect(overrideSummary("Pixel", "calls", 60)).not.toContain("disconnect");
  });

  it("without a remembered child name the wording still reads", () => {
    expect(emergencyControlSummary("Pixel", { childName: "", order: "before" })).toBe(
      "An emergency control was used on Pixel, before you disconnected it.",
    );
    expect(emergencyControlSummary("Pixel", { childName: "", order: "after" })).toBe(
      `An emergency control was used on Pixel, after you disconnected it. ${NOT_RECEIVED}`,
    );
  });
});
