import { describe, expect, it } from "vitest";
import {
  CURSOR_OVERLAP_SECS,
  SWEEP_INTERVAL_SECS,
  WINDOW_FLOOR_SECS,
  nextCursor,
  readWindow,
  sweepDue,
  windowSince,
} from "./relayWindow";

const NOW = 1_800_000_000;

describe("windowSince", () => {
  it("sweeps the whole day when the session has no cursor yet", () => {
    expect(windowSince(null, NOW)).toBe(NOW - WINDOW_FLOOR_SECS);
  });

  it("narrows to the cursor once one exists", () => {
    const cursor = NOW - 300;
    expect(windowSince(cursor, NOW)).toBe(cursor);
  });

  it("never reaches further back than the day floor", () => {
    // A cursor pinned by an ask nobody could ingest, a week ago.
    expect(windowSince(NOW - 7 * 24 * 60 * 60, NOW)).toBe(NOW - WINDOW_FLOOR_SECS);
  });
});

describe("nextCursor", () => {
  it("advances to the read's own start, less the overlap", () => {
    expect(nextCursor(NOW - 600, NOW, false)).toBe(NOW - CURSOR_OVERLAP_SECS);
  });

  it("overlaps rather than resuming exactly where it left off", () => {
    // Two phones do not share a clock, and relays order by the sender's
    // timestamp — so the next window must reach back over the seam.
    expect(nextCursor(null, NOW, false)).toBeLessThan(NOW);
  });

  it("holds the cursor when something could not be ingested", () => {
    const cursor = NOW - 600;
    expect(nextCursor(cursor, NOW, true)).toBe(cursor);
  });

  it("keeps the full sweep when the very first read left work behind", () => {
    // Still null, so the next read is still a whole-day sweep: the ask that
    // could not land is guaranteed to be in it.
    expect(nextCursor(null, NOW, true)).toBeNull();
  });

  it("gives an unlanded ask another chance on the next read", () => {
    // The regression this window must never reintroduce: a ward asks for time
    // from a phone the guardian has not added yet. The read cannot ingest it,
    // so the NEXT read has to cover the same moment again — otherwise she
    // waits and nobody is ever told.
    const askedAt = NOW - 30;
    const cursor = nextCursor(NOW - 600, NOW, true);
    expect(windowSince(cursor, NOW + 30)).toBeLessThanOrEqual(askedAt);
  });
});

describe("sweepDue", () => {
  it("sweeps when none has happened yet", () => {
    expect(sweepDue(null, NOW)).toBe(true);
  });

  it("does not sweep again before the interval is up", () => {
    expect(sweepDue(NOW - SWEEP_INTERVAL_SECS + 1, NOW)).toBe(false);
  });

  it("sweeps once the interval has passed, exactly at it too", () => {
    expect(sweepDue(NOW - SWEEP_INTERVAL_SECS, NOW)).toBe(true);
    expect(sweepDue(NOW - 2 * SWEEP_INTERVAL_SECS, NOW)).toBe(true);
  });

  it("sweeps when forced (foreground, pull-to-refresh) however recent the last one", () => {
    expect(sweepDue(NOW - 5, NOW, true)).toBe(true);
  });
});

describe("readWindow", () => {
  it("a sweep reads from the floor and ignores the cursor", () => {
    const cursor = NOW - 300;
    expect(readWindow(cursor, null, NOW)).toEqual({ since: NOW - WINDOW_FLOOR_SECS, sweep: true });
    expect(readWindow(cursor, NOW - 10, NOW, true)).toEqual({ since: NOW - WINDOW_FLOOR_SECS, sweep: true });
  });

  it("any other read follows the cursor", () => {
    const cursor = NOW - 300;
    expect(readWindow(cursor, NOW - 60, NOW)).toEqual({ since: cursor, sweep: false });
  });

  it("the floor is unchanged at a day", () => {
    expect(WINDOW_FLOOR_SECS).toBe(24 * 60 * 60);
  });
});
