import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  RELEASE_LINK_GRACE_MS,
  endReleaseGrace,
  onReleaseGraceEnd,
  releasingMachines,
  resetReleaseGraceForTests,
  startReleaseGrace,
} from "./releaseGrace";

const M = "a".repeat(64);

describe("the release link grace", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    resetReleaseGraceForTests();
    vi.useRealTimers();
  });

  it("holds a machine for 10 seconds and then tells the listener once", () => {
    const end = vi.fn();
    onReleaseGraceEnd(end);
    startReleaseGrace(M);
    expect(RELEASE_LINK_GRACE_MS).toBe(10_000);
    expect(releasingMachines().has(M)).toBe(true);
    vi.advanceTimersByTime(9_999);
    expect(releasingMachines().has(M)).toBe(true);
    expect(end).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(releasingMachines().has(M)).toBe(false);
    expect(end).toHaveBeenCalledTimes(1);
  });

  it("restarting runs the clock from the new send, with one end", () => {
    const end = vi.fn();
    onReleaseGraceEnd(end);
    startReleaseGrace(M);
    vi.advanceTimersByTime(6_000);
    startReleaseGrace(M);
    vi.advanceTimersByTime(6_000);
    expect(releasingMachines().has(M)).toBe(true);
    vi.advanceTimersByTime(4_000);
    expect(releasingMachines().has(M)).toBe(false);
    expect(end).toHaveBeenCalledTimes(1);
  });

  it("ending it early tells the listener and cancels the timer", () => {
    const end = vi.fn();
    onReleaseGraceEnd(end);
    startReleaseGrace(M);
    endReleaseGrace(M);
    expect(releasingMachines().has(M)).toBe(false);
    expect(end).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(20_000);
    expect(end).toHaveBeenCalledTimes(1);
  });

  it("reads as over at the deadline whatever the timer says", () => {
    startReleaseGrace(M, 1_000);
    expect(releasingMachines(10_999).has(M)).toBe(true);
    expect(releasingMachines(11_000).has(M)).toBe(false);
  });
});
