import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { statusFor, type StatusForResult } from "./statusFor";

/**
 * The state a screen shows for a child. It's either the device's REAL reported
 * state (from the STATUS feed) or, when we have no fresh feed, the locally
 * computed guess (`statusFor`, whose usage is still stubbed).
 */
export interface LiveStatus extends StatusForResult {
  /** True when this reflects the device's real reported state, not a local guess. */
  live: boolean;
  /** Unix seconds of the device feed, when `live`. */
  asOf?: number;
  /** True when the child is currently locked out. */
  locked: boolean;
  /** Minutes of time-free learning today, when the device reports it. */
  learningMinutesToday?: number;
  /**
   * The exact seconds the DEVICE reported left today. Present only on a live
   * feed: the local guess has no usage input, and a made-up countdown is
   * worse than none. Drives the seconds shown in the last stretch.
   */
  secondsLeftToday?: number;
}

/** A device STATUS older than this many seconds is treated as stale. */
export const STATUS_FRESHNESS_SECS = 180;

/**
 * The one report that speaks for a ward across all their devices.
 *
 * **A locked device wins over an unlocked one**, regardless of which beat more
 * recently. Picking purely the newest heartbeat was wrong in the way that
 * matters: decented called a stand-down on 2026-07-27, watched the ward get the
 * warning and the lock, and Kintrinsic said "Can use their screen now — live"
 * and "Allowed now". A second device had simply reported a moment later, and
 * its unlocked answer overwrote the locked one.
 *
 * That is the worst direction for this summary to fail in. Telling a guardian
 * their ward is free when they are locked out contradicts what the ward is
 * looking at, and — right after the guardian deliberately locked them — reads
 * as the action having failed. So ties go to the more restrictive answer, and
 * among equals the freshest still wins.
 *
 * The imprecision this trades for is real and worth naming: a ward whose laptop
 * is outside its hours while their phone is open now summarises as locked. Any
 * clause a guardian sends goes to EVERY device, so devices disagreeing is the
 * transient or the exception, and "locked" is the honest headline for it.
 */
export function freshestStatusFor(
  child: Child,
  deviceStatus: Record<string, DeviceStatus>,
  now: number,
  freshnessSecs: number = STATUS_FRESHNESS_SECS,
): DeviceStatus | undefined {
  const fresh = child.devices
    .filter((d) => d.pairing === "paired" && d.devicePubkey)
    .map((d) => deviceStatus[d.devicePubkey as string])
    .filter((s): s is DeviceStatus => Boolean(s))
    .filter((s) => now / 1000 - s.ts <= freshnessSecs)
    .sort((a, b) => b.ts - a.ts);
  // Stale reports are not evidence of freedom either: fall back to the newest
  // of everything so `liveStatusFor` can judge staleness exactly as before.
  if (fresh.length === 0) {
    return child.devices
      .filter((d) => d.pairing === "paired" && d.devicePubkey)
      .map((d) => deviceStatus[d.devicePubkey as string])
      .filter((s): s is DeviceStatus => Boolean(s))
      .sort((a, b) => b.ts - a.ts)[0];
  }
  return fresh.find((s) => s.locked) ?? fresh[0];
}

/**
 * Prefer the device's real STATUS feed when we have a FRESH one; otherwise fall
 * back to the locally-computed guess. `now` is epoch **ms**; a feed's `ts` is
 * unix **seconds**.
 */
export function liveStatusFor(
  child: Child,
  status: DeviceStatus | undefined,
  now: number,
  freshnessSecs: number = STATUS_FRESHNESS_SECS,
): LiveStatus {
  if (status && now / 1000 - status.ts <= freshnessSecs) {
    // An UNBOUNDED day arrives as 0: STATUS carries unsigned seconds, so the
    // enforcer's -1 ("no limit") saturates on the way out. Taken literally
    // that renders as "about to lock" — the opposite of the truth — for a
    // ward with no clauses at all, or one whose schedule the guardian paused
    // ("allow anytime") with no daily cap. The device names the first case
    // itself (`unconstrained`); for the second, this app signed the charter,
    // so it can tell. A capped ward genuinely at zero is untouched: their own
    // policy still yields a number, so they fall through as before.
    if (!status.locked && status.effectiveSecs === 0) {
      const unbounded =
        status.source === "unconstrained" || statusFor(child, now).minutesLeftToday === null;
      if (unbounded) {
        return {
          allowedNow: true,
          minutesLeftToday: null,
          locked: false,
          live: true,
          asOf: status.ts,
          ...(status.learningTodaySecs != null
            ? { learningMinutesToday: Math.floor(status.learningTodaySecs / 60) }
            : {}),
        };
      }
    }
    return {
      allowedNow: !status.locked,
      // A stand-down waits for a PERSON — the guardian holding this screen —
      // so it must not read as "outside allowed hours". A malformed-clause
      // lock is a fail-safe; show it as a schedule lock rather than leak the
      // internal.
      reason:
        status.lockReason === "budget"
          ? "budget"
          : status.lockReason === "standdown"
            ? "standdown"
            : status.locked
              ? "schedule"
              : undefined,
      minutesLeftToday: Math.floor(status.effectiveSecs / 60),
      secondsLeftToday: status.effectiveSecs,
      locked: status.locked,
      live: true,
      asOf: status.ts,
      ...(status.learningTodaySecs != null
        ? { learningMinutesToday: Math.floor(status.learningTodaySecs / 60) }
        : {}),
    };
  }
  const s = statusFor(child, now);
  return {
    ...s,
    live: false,
    locked: !s.allowedNow && s.minutesLeftToday === 0,
  };
}
