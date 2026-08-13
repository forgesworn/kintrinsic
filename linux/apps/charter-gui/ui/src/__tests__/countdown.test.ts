import { describe, it, expect } from "vitest";
import { Countdown } from "../countdown";

describe("countdown", () => {
  it("interpolates locally between syncs", () => {
    const c = new Countdown();
    c.sync(600, 0); // 10 minutes at t=0
    expect(c.remaining(0)).toBe(600);
    expect(c.remaining(60_000)).toBe(540); // 1 min later
    expect(c.remaining(600_000)).toBe(0); // never negative
    expect(c.remaining(999_000)).toBe(0);
  });

  it("hard re-syncs on every TimeLeftChanged (no drift)", () => {
    const c = new Countdown();
    c.sync(600, 0);
    expect(c.remaining(120_000)).toBe(480);
    // The daemon says 300s remain now (e.g. an extension shrank, or correction).
    c.sync(300, 120_000);
    expect(c.remaining(120_000)).toBe(300);
    expect(c.remaining(180_000)).toBe(240);
  });

  it("treats -1 as unlimited", () => {
    const c = new Countdown();
    c.sync(-1, 0);
    expect(c.remaining(10_000_000)).toBe(-1);
  });
});
