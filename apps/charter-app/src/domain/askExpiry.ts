// Mirrors the ward's own ask TTL, so the guardian never offers a decision the
// device has already stopped listening for.
//
// core/crates/charter-spine/src/broker.rs (`PENDING_TTL_SECS`, commit
// 7ba043b): a Pending record older than 24h ages out to `Expired` (terminal)
// on the device itself, and a GRANT for it is not enacted (002c211, "on_grant
// respects a concurrent expiry"). The guardian's own request queue had no
// matching limit (review 2026-09-27, G-3) — an ask sitting for 30 hours could
// still be approved here, sign a GRANT, log "gave N minutes" in Activity, and
// the device would silently ignore it because its own copy had already gone
// terminal. A guardian who approved a 30-hour-old ask and watched nothing
// happen had no way to know why.

/** Seconds — matches `charter-spine::broker::PENDING_TTL_SECS` exactly. */
export const ASK_EXPIRY_SECS = 24 * 3600;

/**
 * Whether an ask created at `createdAt` (epoch MILLISECONDS — the unit
 * `ChildRequest.createdAt` already uses, itself the ask's own device-clock
 * `ts`) is older than the ward's own 24h TTL as of `now` (epoch ms).
 */
export function isAskExpired(createdAt: number, now: number): boolean {
  return now - createdAt >= ASK_EXPIRY_SECS * 1000;
}
