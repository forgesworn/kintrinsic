// Consuming the machine-authored device AUDIT (kind 31000) — the break-glass
// override the ward raises when they use the emergency unlock
// (`spec/contract.md` "Break-glass override event"). Like STATUS, the rumor is
// UNSIGNED on the wire and the machine's schnorr signature lives on the SEAL, so
// authenticity = a valid machine-signed seal PLUS rumor.pubkey == seal.pubkey.
// nip44 sealing alone proves nothing (anyone can encrypt to the guardian's
// public key and self-declare an author), which is why the seal is verified.
//
// The audit is tags-only (content always empty). The guardian app used to ignore
// it entirely — the carrier raised the notification but the Activity history
// never showed the emergency — so this is the read side of what the ward emits.

import { nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import { MAX_WRAP_JITTER_SECS } from "./request";

export const CHARTER_DEVICE_AUDIT = 31000;
const SEAL = 13;

/**
 * The oldest a press may be, before the wrap that reports it, and still be
 * dated by its own `at`. A reset or wound-back ward clock can stamp `at` as
 * 1970; trusting it would date the line there and bury a real emergency at the
 * bottom of the history. A genuine press that sat offline for over a month is
 * far rarer than a bad clock, so beyond this the wrap's own time is used.
 */
export const MAX_PRESS_AGE_SECS = 30 * 24 * 60 * 60;

export type OverrideScope = "calls" | "full";

/** One emergency unlock, as the ward reported it. */
export interface DeviceOverride {
  /** The device it came from (the rumor author, verified against the seal). */
  machine: string;
  /** What the ward opened; null when the tag is missing or not one we know. */
  scope: OverrideScope | null;
  /** How long it stayed open; null when the tag is missing or unusable. */
  durationSecs: number | null;
  /** Unix SECONDS the ward pressed it (or, for an older ward, when it was reported). */
  at: number;
  /**
   * Identity of this override, for folding copies into one line. When the ward
   * stamped a press time it is `${machine}:${at}`: a wrap rebuilt after a day
   * offline has a new id and a new `created_at` but the same `at`, so the copies
   * collapse. Without one the best available identity is the wrap itself.
   * Identity is separate from dating: it uses the tag even when the value is
   * too odd to date the line by.
   */
  key: string;
}

function tagValue(tags: unknown, name: string): string | undefined {
  if (!Array.isArray(tags)) return undefined;
  for (const t of tags) {
    if (Array.isArray(t) && t[0] === name) return typeof t[1] === "string" ? t[1] : undefined;
  }
  return undefined;
}

/** A plain decimal non-negative integer: no sign, no exponent, no whitespace. */
function parseDecimal(s: string | undefined): number | null {
  if (s === undefined || !/^\d{1,15}$/.test(s)) return null;
  return Number(s);
}

/**
 * Decrypt, verify and parse one gift-wrapped device audit; only a break-glass
 * override (`outcome=override`, `op=unlock.breakglass`) is accepted. Returns
 * `null` on any decrypt / verify / kind / freshness / shape failure (never
 * throws).
 *
 * A malformed scope or duration does NOT reject the event: hiding a real
 * emergency is the worse failure, so it is shown with less detail instead.
 */
export function unwrapAudit(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
  nowSecs: number = Math.floor(Date.now() / 1000),
): DeviceOverride | null {
  try {
    // Jitter: wraps are backdated; drop one too far from now in either direction
    // (the same window charterd's nip59::unwrap enforces on its side).
    if (Math.abs(nowSecs - wrap.created_at) > MAX_WRAP_JITTER_SECS) return null;
    const seal: NostrEvent = JSON.parse(
      nip44.decrypt(wrap.content, nip44.getConversationKey(recipientSk, wrap.pubkey)),
    );
    if (seal.kind !== SEAL) return null;
    // The seal is the machine's SIGNED event — verify it (id + schnorr sig).
    if (!verifyEvent(seal)) return null;
    const rumor: NostrEvent = JSON.parse(
      nip44.decrypt(seal.content, nip44.getConversationKey(recipientSk, seal.pubkey)),
    );
    if (rumor.kind !== CHARTER_DEVICE_AUDIT) return null;
    // The sealed (signed) author must equal the rumor author — no forged sender.
    if (rumor.pubkey !== seal.pubkey) return null;
    if (tagValue(rumor.tags, "outcome") !== "override") return null;
    if (tagValue(rumor.tags, "op") !== "unlock.breakglass") return null;

    const created = rumor.created_at;
    if (typeof created !== "number" || !Number.isInteger(created) || created < 0) return null;

    const scopeTag = tagValue(rumor.tags, "scope");
    const scope: OverrideScope | null = scopeTag === "calls" || scopeTag === "full" ? scopeTag : null;
    const secs = parseDecimal(tagValue(rumor.tags, "durationSecs"));
    const durationSecs = secs !== null && secs > 0 ? secs : null;

    // The press time. A wrap is stamped when it is built, so `created_at` is
    // only a stand-in. Two questions are kept apart:
    //  - identity: any parseable `at` names this press. A rebuilt wrap carries
    //    the same tag, so keying on it keeps the copies one line even when the
    //    value is unfit for dating (a clock stepped back between the press and
    //    the rebuild would otherwise flip a copy to a `wrap:` key and add a
    //    second line).
    //  - dating: `at` is used only when it is not after its own wrap (a press
    //    cannot postdate the wrap reporting it) and not absurdly old.
    const atTag = parseDecimal(tagValue(rumor.tags, "at"));
    const usable = atTag !== null && atTag <= created && created - atTag <= MAX_PRESS_AGE_SECS;
    return {
      machine: rumor.pubkey,
      scope,
      durationSecs,
      at: usable ? atTag : created,
      key: atTag !== null ? `${rumor.pubkey}:${atTag}` : `wrap:${wrap.id}`,
    };
  } catch {
    return null;
  }
}
