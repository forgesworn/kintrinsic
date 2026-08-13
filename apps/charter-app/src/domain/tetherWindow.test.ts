import { describe, expect, it } from "vitest";
import { tetherWindowLabel } from "./tetherWindow";

const NOW = 1_800_000_000;
const EOD = NOW + 6 * 3600; // "end of day" six hours from now

describe("tetherWindowLabel", () => {
  it("no until → Until I turn it off", () => {
    expect(tetherWindowLabel({ allow: "filtered" }, NOW, EOD)).toBe("Until I turn it off");
  });

  it("hour buckets match within tolerance", () => {
    expect(tetherWindowLabel({ allow: "filtered", until: NOW + 3600 }, NOW, EOD)).toBe("1 hour");
    expect(tetherWindowLabel({ allow: "raw", until: NOW + 120 * 60 + 60 }, NOW, EOD)).toBe(
      "2 hours",
    );
  });

  it("REST OF TODAY is selectable (the 2026-07-23 bug)", () => {
    expect(tetherWindowLabel({ allow: "filtered", until: EOD }, NOW, EOD)).toBe("Rest of today");
    expect(tetherWindowLabel({ allow: "filtered", until: EOD - 200 }, NOW, EOD)).toBe(
      "Rest of today",
    );
  });

  it("rest-of-today wins over an hour bucket late in the evening", () => {
    // 60 min before midnight: both "1 hour" and end-of-day describe the same
    // moment — the chip the guardian TAPPED (rest of today) must win.
    const eod = NOW + 3600;
    expect(tetherWindowLabel({ allow: "filtered", until: eod }, NOW, eod)).toBe("Rest of today");
  });

  it("an odd stored until falls back rather than lying with a near bucket", () => {
    expect(tetherWindowLabel({ allow: "filtered", until: NOW + 7 * 3600 }, NOW, EOD)).toBe(
      "Until I turn it off",
    );
  });
});
