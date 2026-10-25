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
  | {
      outcome: "sent";
      /** Clause wraps the carrier took for home-network delivery (see
       *  `localDelivery.ts`); absent when everything went by relays only. */
      wraps?: string[];
    }
  | { outcome: "undelivered"; why: "no-signer" | "no-device" }
  /**
   * Not everywhere yet: the carrier is holding the change for a ward device
   * (the relays failed and the ward is not on the home network, or has not
   * confirmed), and/or a multi-device send did not get to some device, while
   * at least one device reached it or holds it. Saved here and on its way, but
   * NOT sent everywhere: callers must not treat this as `sent`, nor as a
   * failure that invites a second signature (an approval counts as answered).
   * `message` is the guardian-facing sentence.
   */
  | {
      outcome: "queued";
      message: string;
      wraps?: string[];
      /** Names of the devices still waiting for the home network. */
      devices?: string[];
      /** Names of devices in the same send that DID get it (a partial batch). */
      reached?: string[];
      /** One sentence per device the send did not get to (a partial batch). */
      failed?: string[];
    }
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
  if (d.outcome === "queued") {
    if (isSplit(d)) return ` — ${splitSentence(d.reached ?? [], d.devices ?? [], d.failed)}`;
    return ` — saved here. ${d.message.replace(/^Not sent yet — /, "Not sent yet: ")}`;
  }
  if (d.outcome !== "undelivered") return "";
  return d.why === "no-signer"
    ? " — saved here, but not sent yet: turn on parent approval to send it"
    : " — saved here, but their phone isn’t set up in this app yet, so it hasn’t been sent";
}

/** "A", "A and B", "A, B and C". */
export function joinNames(names: string[]): string {
  const n = [...new Set(names)];
  if (n.length <= 1) return n[0] ?? "";
  return `${n.slice(0, -1).join(", ")} and ${n[n.length - 1]}`;
}

/**
 * The honest split for a multi-device send where some devices got it and the
 * rest are being held for the home network. Used for the activity line and
 * for what the guardian is told at the time.
 */
export function splitSentence(reached: string[], waiting: string[], failed: string[] = []): string {
  const parts: string[] = [];
  if (reached.length > 0) parts.push(`Reached ${joinNames(reached)}.`);
  const w = [...new Set(waiting)];
  if (w.length > 0) {
    parts.push(`${joinNames(w)} will get it when ${w.length > 1 ? "they're" : "it's"} on the home network.`);
  }
  parts.push(...failed);
  return parts.join(" ");
}

/** A queued outcome that is a real split (some reached, or some failed), not
 *  simply "everything is waiting for the home network". */
function isSplit(d: Extract<ClauseDelivery, { outcome: "queued" }>): boolean {
  return (
    (d.failed !== undefined && d.failed.length > 0) ||
    (d.reached !== undefined && d.reached.length > 0 && d.devices !== undefined && d.devices.length > 0)
  );
}

/**
 * The warning shown on a rule screen when the save was kept here and handed to
 * the carrier for the home network (not an error, and not done yet). Null for
 * every other outcome.
 */
export function queuedSaveWarning(d: ClauseDelivery): string | null {
  if (d.outcome !== "queued") return null;
  const failed = d.failed ?? [];
  const held = d.devices && d.devices.length > 0 ? d.devices : failed.length > 0 ? [] : ["their device"];
  const parts = ["Saved here."];
  const n = [...new Set(held)];
  if (n.length > 0) {
    parts.push(`It'll reach ${joinNames(n)} when ${n.length > 1 ? "they're" : "it's"} on the home network.`);
  }
  if (d.reached && d.reached.length > 0) parts.push(`${joinNames(d.reached)} already got it.`);
  parts.push(...failed);
  return parts.join(" ");
}

/**
 * What `giveTime` / `standDown` / the install window / an update push report.
 * `true` = sent; `false` = not signed. A "queued" outcome is truthy-by-shape:
 * the change IS signed and on its way, but not everywhere yet — at least one
 * device is waiting for the home network, or did not get it — and `message`
 * says exactly who got it, who is still waiting and who it did not reach — for
 * the screen to show. Never present it as done.
 */
export type SendResult = boolean | { outcome: "queued"; message: string };

/** The sentence a screen shows after a send that is not everywhere yet. */
export function queuedSendLine(d: Extract<ClauseDelivery, { outcome: "queued" }>): string {
  return isSplit(d) ? splitSentence(d.reached ?? [], d.devices ?? [], d.failed) : d.message;
}

/**
 * The tail of an activity line for an ANSWER (a signed grant or refusal) that
 * the carrier is holding for the home network. The answer exists; it just
 * hasn't been delivered yet.
 */
export function waitingAnswerNote(devices: string[]): string {
  const names = devices.length > 0 ? devices : ["their device"];
  return ` — waiting for ${joinNames(names)} to join the home network`;
}
