import { describe, expect, it } from "vitest";
import {
  PULL_MAX,
  PULL_THRESHOLD,
  pullOffset,
  willRefresh,
} from "./pullGesture";

describe("pull-to-refresh feel", () => {
  it("ignores upward and zero travel", () => {
    expect(pullOffset(0)).toBe(0);
    expect(pullOffset(-50)).toBe(0);
  });

  it("follows the finger at a damped rate", () => {
    // Resisted, not loose: the content moves less than the finger does.
    expect(pullOffset(40)).toBeLessThan(40);
    expect(pullOffset(40)).toBeGreaterThan(0);
  });

  it("caps the pull so a long drag can't push the content off-screen", () => {
    expect(pullOffset(10_000)).toBe(PULL_MAX);
  });

  it("needs a deliberate pull before a release refreshes", () => {
    // A stray few px while starting a scroll must not fire a fetch.
    expect(willRefresh(pullOffset(8))).toBe(false);
    expect(willRefresh(PULL_THRESHOLD - 1)).toBe(false);
    expect(willRefresh(PULL_THRESHOLD)).toBe(true);
  });

  it("can always reach the threshold — the cap is above it", () => {
    // Guards the pair of constants against drifting past each other, which
    // would make the gesture impossible to complete.
    expect(PULL_MAX).toBeGreaterThanOrEqual(PULL_THRESHOLD);
    expect(willRefresh(pullOffset(10_000))).toBe(true);
  });
});
