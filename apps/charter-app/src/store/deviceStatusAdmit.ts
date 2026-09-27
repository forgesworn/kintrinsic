// Admitting a freshly-decrypted STATUS into the guardian's live map.
//
// SECOND ROUND (review 2026-09-27, F1/F2): the first round made the newest
// `ts` win over the stored current reading whenever it was LOWER too,
// reasoning that a lower `ts` was the only signal a ward's forced
// clock-backwards emit (`core/crates/charter-spine/src/status_emit.rs`,
// `ed79b79`) could produce. That broke two things at once:
//
//  - F1: `pollRelayOnce`'s first sweep of a session reads the whole 24h
//    window and relays do not serve it chronologically, so an ordinary
//    OLDER-but-forward-clocked heartbeat routinely arrives after a newer one.
//    Accepting it as current — every session, on every reload — pinned the
//    guardian's view to a stale reading (hidden pauses, stale versions, wrong
//    "standing") until the next heartbeat happened to land.
//  - F2: nothing bound a wrap to "now" except the ±2-day jitter check, so
//    ANY relay could re-serve an old, genuine wrap and pin the guardian to
//    stale state indefinitely, with "clock went backwards" shown throughout.
//
// The fix was the agreed wire addition: `StatusPayload.clockSteppedBackFrom`
// (optional, unix seconds), the WARD's own signed marker that a lower `ts`
// is a genuine clock step, not a replay. That closed F1/F2, but left one
// hole (R2-2/R2-3 below): the marker's SIZE is ward-controlled and reusable
// — anyone can re-publish a genuine old step-back STATUS, and for as long as
// its marker still reaches the currently-stored `ts` (bounded only by the
// ±2-day wrap jitter, review `MAX_WRAP_JITTER_SECS`), every guardian admits
// it as current again, displacing a newer reading. A restart also drops the
// in-memory marker on the ward, freezing the guardian's view for up to that
// same ~2 days after a forced reboot.
//
// THIRD ROUND (review round 2, R2-2/R2-3): the agreed fix is a monotonic
// per-device `seq` (`StatusPayload.seq`, `wire/status.ts`). A ward that
// sends it guarantees it strictly increases across restarts, including a
// wipe — so a replay's `seq` can never be newer than what it already lost
// to, closing the hole `ts` alone left open. Rules, in order:
//
//  1. BOTH readings carry `seq`: ordering is by `seq` alone — `ts` is not
//     consulted for ordering at all. A strictly greater `seq` is admitted;
//     an equal or lesser one is a duplicate/replay and changes nothing.
//  2. Only the INCOMING reading carries `seq` (the ward just upgraded):
//     there is nothing on the stored reading to compare `seq` against, so
//     admission falls back to a loosened `ts` rule — forward or equal `ts`
//     admits outright, and a `ts` up to five minutes BEHIND the stored one
//     also admits (the upgrade itself, an app restart, can land a hair
//     behind the last pre-upgrade heartbeat). The existing ward-vouched-
//     marker rule (3) still admits beyond that tolerance.
//  3. Only the STORED reading carries `seq` (a replay from before the ward
//     adopted it, or a downgraded ward that has lost its counter): never
//     current — a bare `ts`, however new, proves nothing once this device
//     is known to carry a sequence. Logged once, since it says either the
//     relay is serving stale traffic or a ward has regressed.
//  4. NEITHER carries `seq` (an older ward, e.g. Android until it adopts
//     this): the second-round `ts` + `clockSteppedBackFrom` rule, unchanged.
//
// Whenever a reading is admitted via `seq` (rules 1 or 2), the "clock went
// backwards" note is still set exactly as before — when the admitted status
// carries `clockSteppedBackFrom`, OR its `ts` is below the reading it
// displaced — and clears per the existing rule (`carryOrClearFlag` below).
// `clockSteppedBackFrom` therefore still exists and still drives that note;
// it simply no longer decides WHICH reading is current once `seq` can.
import type { DeviceStatus } from "../wire/status";

/** The live STATUS map's value shape: the wire payload plus a guardian-local,
 *  never-transmitted note about how it got here. */
export type LiveDeviceStatus = DeviceStatus & {
  /** True while the currently-stored reading followed a ward-vouched
   *  backward clock step (see the file doc) and the clock has not yet been
   *  seen to catch back up. Never present on a `DeviceStatus` fresh off the
   *  wire; set and cleared only by `mergeDeviceStatus`. */
  clockWentBackwards?: boolean;
  /** Guardian-local wall clock (ms, `Date.now()`) at the moment the flag was
   *  first set — the anchor for the 24h floor below. Carried forward on
   *  every subsequent forward-clocked reading while the flag stays set;
   *  absent whenever `clockWentBackwards` is absent. */
  clockWentBackwardsAt?: number;
};

/** How long `clockWentBackwards` may outlive the step it reported, even if
 *  the ward's clock never visibly catches back up to `clockSteppedBackFrom`
 *  (it could have been corrected to a still-earlier time and then drift up
 *  slowly, or simply never reconnect). Chosen so the note cannot become a
 *  permanent fixture of a device's card over one bad clock event: it clears
 *  itself out within a day either way. */
const FLAG_MAX_AGE_MS = 24 * 3600 * 1000;

/** How far BEHIND the stored `ts` an incoming reading may land and still be
 *  admitted purely on the strength of just having upgraded to `seq` (rule 2
 *  in the file doc) — the tolerance for the upgrade itself (an app restart)
 *  landing a hair behind the last pre-upgrade heartbeat. Anything further
 *  behind still needs the ward-vouched marker rule to be admitted. */
const UPGRADE_TS_TOLERANCE_SECS = 5 * 60;

/**
 * Carry the `clockWentBackwards` flag onto an ORDINARY forward-clocked
 * `status` (its `ts` is strictly greater than `cur.ts`), or let it clear.
 *
 * The flag clears — the reading becomes an ordinary, unflagged one — at
 * whichever of these comes first:
 *  - the new `ts` reaches or passes `cur.clockSteppedBackFrom` (the ward's
 *    clock is back past where it was when it stepped back), or
 *  - `FLAG_MAX_AGE_MS` has elapsed since the flag was first set.
 *
 * `cur.clockSteppedBackFrom` — the wire field — survives on `cur` because it
 * was part of the payload that first got flagged, even though an ordinary
 * follow-up heartbeat never repeats it itself; it must be re-stamped onto
 * the merged result below for as long as the flag persists, or the next
 * heartbeat after this one would have nothing left to compare against.
 */
function carryOrClearFlag(
  cur: LiveDeviceStatus,
  status: DeviceStatus,
  nowMs: number,
): LiveDeviceStatus {
  if (!cur.clockWentBackwards) return status;
  const threshold = cur.clockSteppedBackFrom;
  const caughtUp = threshold !== undefined && status.ts >= threshold;
  const timedOut =
    cur.clockWentBackwardsAt !== undefined && nowMs - cur.clockWentBackwardsAt >= FLAG_MAX_AGE_MS;
  if (caughtUp || timedOut) return status;
  return {
    ...status,
    clockSteppedBackFrom: threshold,
    clockWentBackwards: true,
    clockWentBackwardsAt: cur.clockWentBackwardsAt,
  };
}

/**
 * Admit `status` as current — admission itself was already decided by the
 * caller (`seq` order, or the upgrade-transition rule) — and either stamp
 * the backward-clock notice fresh, or hand off to `carryOrClearFlag` when
 * this admission is not itself a backward step.
 *
 * The notice is set fresh whenever the admitted status carries
 * `clockSteppedBackFrom`, OR its `ts` is below `cur.ts` (the reading it is
 * displacing) — `seq` having already proved this is genuinely the newer
 * reading, unlike the pre-`seq` rule, no `>= cur.ts` check on the marker's
 * size is needed here.
 */
function admitWithBackwardCheck(
  cur: LiveDeviceStatus,
  status: DeviceStatus,
  nowMs: number,
): LiveDeviceStatus {
  const wentBackwardsNow = status.clockSteppedBackFrom !== undefined || status.ts < cur.ts;
  if (!wentBackwardsNow) return carryOrClearFlag(cur, status, nowMs);
  return {
    ...status,
    clockSteppedBackFrom: status.clockSteppedBackFrom ?? cur.ts,
    clockWentBackwards: true,
    clockWentBackwardsAt: nowMs,
  };
}

/**
 * The pre-`seq` rule (second round, F1/F2), untouched: used whenever NEITHER
 * reading carries `seq` — an older ward, e.g. Android until it adopts it.
 *
 * The stored current reading is still whichever has the LARGER `ts` —
 * UNLESS a lower-`ts` STATUS carries `clockSteppedBackFrom >= cur.ts` (proof
 * the ward itself is vouching that THIS is the moment its clock stepped
 * back past what's currently shown). Any other lower-`ts` STATUS (no
 * marker, or a stale one) is a replay or an out-of-order delivery and is
 * ignored. Equal `ts` is always a no-op.
 */
function admitByTsRule(
  cur: LiveDeviceStatus,
  status: DeviceStatus,
  nowMs: number,
): LiveDeviceStatus {
  if (status.ts > cur.ts) return carryOrClearFlag(cur, status, nowMs);
  if (status.ts === cur.ts) return cur;
  if (status.clockSteppedBackFrom !== undefined && status.clockSteppedBackFrom >= cur.ts) {
    return { ...status, clockWentBackwards: true, clockWentBackwardsAt: nowMs };
  }
  return cur;
}

/**
 * The upgrade transition (rule 2 in the file doc): `status` is the first
 * reading from this machine to carry `seq`, and `cur` has none, so there is
 * nothing to compare `seq` against. Forward or equal `ts` admits outright; a
 * `ts` up to `UPGRADE_TS_TOLERANCE_SECS` behind `cur.ts` also admits, and
 * the ward-vouched marker rule admits beyond that. Anything else is a
 * replay from before the upgrade and is ignored.
 */
function admitOnUpgrade(
  cur: LiveDeviceStatus,
  status: DeviceStatus,
  nowMs: number,
): LiveDeviceStatus {
  if (status.ts >= cur.ts) return admitWithBackwardCheck(cur, status, nowMs);
  const withinUpgradeTolerance = status.ts >= cur.ts - UPGRADE_TS_TOLERANCE_SECS;
  const vouched =
    status.clockSteppedBackFrom !== undefined && status.clockSteppedBackFrom >= cur.ts;
  if (withinUpgradeTolerance || vouched) return admitWithBackwardCheck(cur, status, nowMs);
  return cur;
}

/**
 * What to store for this machine given what's there now (`cur`, possibly
 * absent) and a freshly authenticated, freshly parsed `status`.
 *
 * Returns `cur` itself (by reference) when nothing should change, so a
 * caller can skip the `setState` entirely on a no-op.
 */
export function mergeDeviceStatus(
  cur: LiveDeviceStatus | undefined,
  status: DeviceStatus,
  nowMs: number = Date.now(),
): LiveDeviceStatus {
  if (!cur) return status;

  if (status.seq !== undefined && cur.seq !== undefined) {
    // Rule 1: ordering is by seq alone. Equal is a duplicate re-delivery;
    // lower is a replay or out-of-order delivery — ts is not consulted
    // either way, closing R2-2 (a replayed marker cannot out-rank a seq
    // the current reading already moved past).
    if (status.seq <= cur.seq) return cur;
    return admitWithBackwardCheck(cur, status, nowMs);
  }

  if (status.seq !== undefined && cur.seq === undefined) {
    // Rule 2: the ward just upgraded to seq.
    return admitOnUpgrade(cur, status, nowMs);
  }

  if (status.seq === undefined && cur.seq !== undefined) {
    // Rule 3: a replay from before the upgrade, or a downgraded ward that
    // has lost its counter. Never current.
    console.warn(
      `Kintrinsic: device ${status.machine} sent a STATUS with no seq while a sequenced reading is stored for it; ignoring as stale.`,
    );
    return cur;
  }

  // Rule 4: neither has seq — the second-round ts rule, unchanged.
  return admitByTsRule(cur, status, nowMs);
}
