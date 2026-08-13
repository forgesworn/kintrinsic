import { describe, expect, it } from "vitest";
import { LIVE_WITHIN_MS, liveness, livenessChip, seenLabel } from "./liveness";

const NOW = 1_782_752_400_000;

describe("what 'Connected' means", () => {
  /**
   * The whole point of the change: it must mean the device is THERE, not that
   * someone once finished setting it up. Home used to render a hardcoded
   * "Connected" pill — it said Connected with no device paired at all.
   */
  it("is about being reachable now, not about being configured", () => {
    const longGone = { paired: true, lastSeenAt: NOW - 7 * 24 * 3600 * 1000 };
    expect(liveness(longGone, NOW).kind).toBe("quiet");
    expect(livenessChip(liveness(longGone, NOW), NOW).text).not.toBe("Connected");
  });

  it("tolerates a missed heartbeat without crying offline", () => {
    // STATUS is a 60s heartbeat, so a device three minutes quiet is ordinary.
    const dozing = { paired: true, lastSeenAt: NOW - 3 * 60 * 1000 };
    expect(liveness(dozing, NOW).kind).toBe("live");
  });

  it("draws the line at five minutes", () => {
    expect(liveness({ paired: true, lastSeenAt: NOW - LIVE_WITHIN_MS }, NOW).kind).toBe("live");
    expect(liveness({ paired: true, lastSeenAt: NOW - LIVE_WITHIN_MS - 1 }, NOW).kind).toBe(
      "quiet",
    );
  });

  it("distinguishes never-heard-from from gone quiet", () => {
    // A device that has never reported isn't broken, it just hasn't spoken —
    // saying "Offline" there would send a guardian hunting a fault that isn't
    // one.
    expect(liveness({ paired: true }, NOW).kind).toBe("never-seen");
    expect(livenessChip(liveness({ paired: true }, NOW), NOW).tone).toBe("neutral");
  });

  it("says 'Not set up' when there is nothing paired", () => {
    expect(liveness({ paired: false, lastSeenAt: NOW }, NOW).kind).toBe("not-set-up");
  });

  it("names WHEN a quiet device was last seen, not just that it's quiet", () => {
    const chip = livenessChip(liveness({ paired: true, lastSeenAt: NOW - 3600 * 1000 }, NOW), NOW);
    expect(chip.text).toBe("seen 1 h ago");
    expect(chip.tone).toBe("warn");
  });

  it("counts time in units a parent would use", () => {
    expect(seenLabel(NOW - 30 * 1000, NOW)).toBe("seen moments ago");
    expect(seenLabel(NOW - 20 * 60 * 1000, NOW)).toBe("seen 20 min ago");
    expect(seenLabel(NOW - 5 * 3600 * 1000, NOW)).toBe("seen 5 h ago");
    expect(seenLabel(NOW - 3 * 86400 * 1000, NOW)).toBe("not seen for 3 d");
    // A clock that jumped backwards must not print a negative age.
    expect(seenLabel(NOW + 10_000, NOW)).toBe("seen moments ago");
  });
});
