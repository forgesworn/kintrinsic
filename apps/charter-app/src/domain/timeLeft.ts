// How long is left, said the way the ward's own Kintrinsic app says it.
//
// Kintrinsic showed whole minutes only, so the last stretch — exactly when a
// parent is watching the clock alongside the child — sat still on "2m" for a
// minute at a time. Hours and minutes for the body of the day; minutes AND
// seconds once the end is in sight.

/** Below this many seconds left, the seconds are what matter. */
export const SECONDS_MATTER_BELOW = 180;

/**
 * What is left NOW, counted down from the instant the device reported it.
 *
 * A STATUS heartbeat is throttled to ~60s, so rendering its number verbatim
 * would freeze the second hand — the very thing seconds exist to show. The
 * extrapolation only ever REMOVES time: a device clock ahead of this phone's
 * must not hand the ward minutes nobody granted, and the count stops at zero
 * rather than going negative. `undefined` in (no live report) is `undefined`
 * out — the local guess must not pretend to a countdown.
 */
export function remainingNow(
  reportedSecs: number | undefined,
  asOfUnix: number | undefined,
  nowMs: number,
): number | undefined {
  if (reportedSecs == null) return undefined;
  if (asOfUnix == null) return reportedSecs;
  const elapsed = Math.max(0, nowMs / 1000 - asOfUnix);
  return Math.max(0, reportedSecs - elapsed);
}

/**
 * `1h 05m` · `45m` · `2m 30s` · `45s`.
 *
 * At or below zero it says `<1m` rather than a precise-looking "0s": a device
 * report is up to a heartbeat old, so we genuinely do not know whether the
 * lock has landed yet, and the honest form is the one that does not
 * contradict whatever the child is looking at. Kept short because this lands
 * in the card's 44px type, where prose would run off a phone screen.
 */
export function formatTimeLeft(secs: number): string {
  if (secs <= 0) return "<1m";
  const whole = Math.floor(secs);
  if (whole >= 3600) {
    const h = Math.floor(whole / 3600);
    const m = Math.floor((whole % 3600) / 60);
    return `${h}h ${String(m).padStart(2, "0")}m`;
  }
  if (whole >= SECONDS_MATTER_BELOW) return `${Math.floor(whole / 60)}m`;
  if (whole >= 60) {
    const m = Math.floor(whole / 60);
    return `${m}m ${String(whole % 60).padStart(2, "0")}s`;
  }
  return `${whole}s`;
}
