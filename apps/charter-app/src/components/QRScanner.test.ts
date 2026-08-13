import { describe, expect, it } from "vitest";
import { initialZoom, nextZoom, scanSourceRect, PREFERRED_ZOOM } from "./QRScanner";

describe("scanSourceRect", () => {
  it("returns the full frame at zoom 1", () => {
    expect(scanSourceRect(1920, 1080, 1)).toEqual({ sx: 0, sy: 0, sw: 1920, sh: 1080 });
  });

  it("center-crops at digital zoom 2", () => {
    expect(scanSourceRect(1920, 1080, 2)).toEqual({ sx: 480, sy: 270, sw: 960, sh: 540 });
  });

  it("clamps nonsense zoom values into [1, 6]", () => {
    expect(scanSourceRect(1000, 1000, Number.NaN).sw).toBe(1000);
    expect(scanSourceRect(1000, 1000, 0).sw).toBe(1000);
    expect(scanSourceRect(1000, 1000, 99).sw).toBeCloseTo(1000 / 6);
  });

  it("never goes negative on zero-size video", () => {
    expect(scanSourceRect(0, 0, 2)).toEqual({ sx: 0, sy: 0, sw: 0, sh: 0 });
  });
});

describe("initialZoom", () => {
  it("prefers the sweet spot when the range allows it", () => {
    expect(initialZoom({ min: 1, max: 4 })).toBe(PREFERRED_ZOOM);
  });

  it("clamps to the camera's max when the range is narrow", () => {
    expect(initialZoom({ min: 1, max: 1.2 })).toBe(1.2);
  });

  it("falls back to at least 1 with no range", () => {
    expect(initialZoom(null, 0.5)).toBe(1);
  });
});

describe("nextZoom", () => {
  const range = { min: 1, max: 3, step: 0.1 };

  it("steps by at least the button step", () => {
    expect(nextZoom(1, range, 1)).toBe(1.2);
    expect(nextZoom(1.2, range, -1)).toBe(1);
  });

  it("clamps at both ends of the range", () => {
    expect(nextZoom(2.95, range, 1)).toBe(3);
    expect(nextZoom(1.05, range, -1)).toBe(1);
  });
});
