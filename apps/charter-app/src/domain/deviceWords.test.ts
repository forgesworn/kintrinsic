import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import { deviceNoun, deviceScopeNote } from "./deviceWords";

const dev = (id: string, platform: Device["platform"], label = id): Device => ({
  id,
  label,
  platform,
  pairing: "paired",
  devicePubkey: "a".repeat(64),
});

describe("deviceNoun", () => {
  it("names each platform the way a family does", () => {
    expect(deviceNoun("android", 1)).toBe("phone");
    expect(deviceNoun("linux", 1)).toBe("computer");
    expect(deviceNoun("android", 2)).toBe("phones");
    expect(deviceNoun("linux", 3)).toBe("computers");
  });
});

describe("deviceScopeNote", () => {
  /** The label is the parent's own words — echoed verbatim, never re-typeset. */
  it("says what a single device actually is, by name", () => {
    expect(deviceScopeNote("Robin", [dev("d1", "linux", "Robin's laptop")])).toBe(
      "These rules cover Robin's laptop.",
    );
  });

  /**
   * The bug this exists to kill: a phone is a computer to us and NOT to a
   * parent. "2 of Robin's computers" reads as two laptops when one of them is
   * the phone in his pocket.
   */
  it("names both kinds when the devices differ", () => {
    const note = deviceScopeNote("Robin", [dev("d1", "linux"), dev("d2", "android")]);
    expect(note).toBe("These rules apply to both Robin’s phone and computer.");
  });

  it("puts the phone first regardless of pairing order", () => {
    const note = deviceScopeNote("Robin", [dev("d1", "android"), dev("d2", "linux")]);
    expect(note).toBe("These rules apply to both Robin’s phone and computer.");
  });

  it("uses the plain plural when both devices are the same kind", () => {
    expect(deviceScopeNote("Robin", [dev("d1", "linux"), dev("d2", "linux")])).toBe(
      "These rules apply to both of Robin’s computers.",
    );
    expect(deviceScopeNote("Robin", [dev("d1", "android"), dev("d2", "android")])).toBe(
      "These rules apply to both of Robin’s phones.",
    );
  });

  it("counts, and still breaks down the kinds, beyond two", () => {
    const note = deviceScopeNote("Robin", [
      dev("d1", "android"),
      dev("d2", "android"),
      dev("d3", "linux"),
    ]);
    expect(note).toBe("These rules apply to all 3 of Robin’s devices — 2 phones and 1 computer.");
  });

  it("uses the plain plural beyond two when the kinds match", () => {
    const note = deviceScopeNote("Robin", [
      dev("d1", "linux"),
      dev("d2", "linux"),
      dev("d3", "linux"),
    ]);
    expect(note).toBe("These rules apply to all 3 of Robin’s computers.");
  });

  it("is honest — not silently empty — before anything is paired", () => {
    expect(deviceScopeNote("Robin", [])).toBe(
      "These rules will apply once you pair a device for Robin.",
    );
  });
});
