import { describe, expect, it } from "vitest";
import { LIVE_WITHIN_MS, anyOnHomeNetwork, liveness, livenessChip, seenLabel } from "./liveness";

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

describe("home-network chip", () => {
  it("says 'On home network' for a paired device with a live session, whatever the heartbeat says", () => {
    const now = 10_000_000;
    const quiet = liveness({ paired: true, lastSeenAt: now - 3 * 3600 * 1000 }, now);
    expect(livenessChip(quiet, now)).toMatchObject({ tone: "warn" });
    expect(livenessChip(quiet, now, true)).toEqual({ text: "On home network", tone: "ok" });
    expect(livenessChip(liveness({ paired: true }, now), now, true)).toEqual({ text: "On home network", tone: "ok" });
    expect(livenessChip(liveness({ paired: true, lastSeenAt: now }, now), now, true).text).toBe("On home network");
  });

  it("never claims it for a device that isn't set up", () => {
    expect(livenessChip(liveness({ paired: false }, 1), 1, true)).toEqual({ text: "Not set up", tone: "neutral" });
  });

  it("anyOnHomeNetwork matches paired devices by machine key only", () => {
    const sessions = new Set(["a".repeat(64)]);
    expect(anyOnHomeNetwork([{ pairing: "paired", devicePubkey: "a".repeat(64) }], sessions)).toBe(true);
    expect(anyOnHomeNetwork([{ pairing: "unpaired", devicePubkey: "a".repeat(64) }], sessions)).toBe(false);
    expect(anyOnHomeNetwork([{ pairing: "paired", devicePubkey: "b".repeat(64) }], sessions)).toBe(false);
    expect(anyOnHomeNetwork([{ pairing: "paired", devicePubkey: null }], sessions)).toBe(false);
  });
});
