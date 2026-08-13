// Whether a signed rule change actually reached the ward — and how to say so.
//
// A draft is a real feature: rules legitimately predate the phone (write the
// charter, then pair). What is NOT acceptable is a draft that reads as done.
// The store used to return a bare `true` both for "signed and sent" and for
// "saved here, nothing left the building", so every caller logged "You updated
// the daily time limit" either way. decented set Mia's limit to 15 minutes
// on 2026-07-27 against a ward whose phone Kintrinsic held no device record for:
// it saved, it read as done, and the phone never heard a word.
//
// Split out of store.tsx so the decision and the wording are unit-testable
// without standing up the whole React provider.

/**
 * What became of a signed rule change. `undelivered` means it is saved locally
 * and the ward's device has NOT been told.
 */
export type ClauseDelivery =
  | { outcome: "sent" }
  | { outcome: "undelivered"; why: "no-signer" | "no-device" }
  | { outcome: "cancelled" };

/**
 * Decide, BEFORE signing, whether a clause has anywhere to go — so nobody is
 * asked to approve a change that cannot travel. Null = go ahead and sign.
 *
 * `deviceCount` is the number of devices with a real (hex) key that clauses can
 * be sealed to, i.e. what `resolveChildTarget` already filters down to; a phone
 * that is paired on its OWN side but missing from the guardian's record counts
 * as zero, which is exactly the split-brain case that started this.
 */
export function clauseDeliveryPrecheck(
  signerConnected: boolean,
  deviceCount: number,
): Extract<ClauseDelivery, { outcome: "undelivered" }> | null {
  if (!signerConnected) return { outcome: "undelivered", why: "no-signer" };
  if (deviceCount === 0) return { outcome: "undelivered", why: "no-device" };
  return null;
}

/**
 * The tail of an activity line when a rule was saved but hasn't travelled.
 * Names the remedy, because "not sent" without "here's why" only worries a
 * parent who can't act on it — and the two causes need different moves.
 * Empty string when the change really did go, so the happy path reads clean.
 */
export function undeliveredNote(d: ClauseDelivery): string {
  if (d.outcome !== "undelivered") return "";
  return d.why === "no-signer"
    ? " — saved here, but not sent yet: turn on parent approval to send it"
    : " — saved here, but their phone isn’t set up in this app yet, so it hasn’t been sent";
}
