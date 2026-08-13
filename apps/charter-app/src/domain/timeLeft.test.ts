import { describe, expect, it } from "vitest";
import { formatTimeLeft, remainingNow, SECONDS_MATTER_BELOW } from "./timeLeft";

describe("remainingNow", () => {
  const ASOF = 1_700_000_000; // unix seconds

  // A heartbeat is up to a minute old and throttled: rendering its number
  // verbatim would freeze the second hand, which is the whole point of
  // showing seconds at all.
  it("counts down from the moment the device reported", () => {
    expect(remainingNow(150, ASOF, ASOF * 1000)).toBe(150);
    expect(remainingNow(150, ASOF, (ASOF + 30) * 1000)).toBe(120);
  });

  it("never runs past zero into negative time", () => {
    expect(remainingNow(150, ASOF, (ASOF + 600) * 1000)).toBe(0);
  });

  // A device clock ahead of this phone's would otherwise ADD time.
  it("does not gain time when the clocks disagree", () => {
    expect(remainingNow(150, ASOF, (ASOF - 90) * 1000)).toBe(150);
  });

  it("has nothing to say without a live report", () => {
    expect(remainingNow(undefined, ASOF, ASOF * 1000)).toBeUndefined();
    // No timestamp to count from: report what was said, unextrapolated.
    expect(remainingNow(150, undefined, ASOF * 1000)).toBe(150);
  });
});

describe("formatTimeLeft", () => {
  // Matches the ward's own Kintrinsic app so a parent and child reading their
  // two screens see the same shape of answer, not two dialects.
  it("reads hours and minutes for a long afternoon", () => {
    expect(formatTimeLeft(90 * 60)).toBe("1h 30m");
    expect(formatTimeLeft(65 * 60)).toBe("1h 05m");
    expect(formatTimeLeft(2 * 3600)).toBe("2h 00m");
  });

  it("reads plain minutes in the ordinary middle", () => {
    expect(formatTimeLeft(45 * 60)).toBe("45m");
    expect(formatTimeLeft(SECONDS_MATTER_BELOW)).toBe("3m");
  });

  // The point of the change: in the last stretch a parent is watching the
  // clock with the child, and "2m" standing still for a minute is useless.
  it("shows the seconds once the end is in sight", () => {
    expect(formatTimeLeft(SECONDS_MATTER_BELOW - 1)).toBe("2m 59s");
    expect(formatTimeLeft(150)).toBe("2m 30s");
    expect(formatTimeLeft(61)).toBe("1m 01s");
  });

  it("drops to bare seconds in the last minute", () => {
    expect(formatTimeLeft(45)).toBe("45s");
    expect(formatTimeLeft(1)).toBe("1s");
  });

  // At zero we genuinely do not know whether the device has locked yet — the
  // report is up to a heartbeat old. Say "less than a minute" rather than a
  // precise-looking "0s", and say it short: this lands in 44px type on a
  // phone, where prose would run off the card.
  it("admits uncertainty at the bottom rather than claiming zero", () => {
    expect(formatTimeLeft(0)).toBe("<1m");
    expect(formatTimeLeft(-30)).toBe("<1m");
  });

  /** Every rendered form has to fit the card's big type on a narrow phone. */
  it("stays short enough for the card at every magnitude", () => {
    for (const secs of [-1, 0, 1, 59, 60, 179, 180, 3599, 3600, 86_399]) {
      expect(formatTimeLeft(secs).length).toBeLessThanOrEqual(7);
    }
  });
});
