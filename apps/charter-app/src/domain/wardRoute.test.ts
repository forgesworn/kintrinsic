import { describe, expect, it } from "vitest";
import { wardIdFromHash } from "./wardRoute";

describe("reading the ward out of the address", () => {
  it("finds the ward on its own tab's route", () => {
    expect(wardIdFromHash("limits", "#/limits/child_sam")).toBe("child_sam");
    expect(wardIdFromHash("activity", "#/activity/child_sam")).toBe("child_sam");
  });

  /**
   * The tab has to match. `#/limits/child_sam` must not select a ward on the
   * Activity screen — both screens read the same hash on every hashchange, and
   * a cross-tab match would have one screen silently follow the other's
   * navigation.
   */
  it("ignores a route belonging to another tab", () => {
    expect(wardIdFromHash("activity", "#/limits/child_sam")).toBe("");
    expect(wardIdFromHash("limits", "#/activity/child_sam")).toBe("");
  });

  it("returns nothing for a bare tab route", () => {
    expect(wardIdFromHash("limits", "#/limits")).toBe("");
    expect(wardIdFromHash("limits", "#/limits/")).toBe("");
    expect(wardIdFromHash("limits", "")).toBe("");
    expect(wardIdFromHash("limits", "#/")).toBe("");
  });

  /** Ids are encoded when written, so they must be decoded when read. */
  it("decodes an escaped id", () => {
    expect(wardIdFromHash("limits", "#/limits/child%20sam")).toBe("child sam");
  });

  it("tolerates a missing leading slash", () => {
    expect(wardIdFromHash("limits", "#limits/child_sam")).toBe("child_sam");
  });
});
