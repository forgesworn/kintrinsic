// The Android guardian PIN (contract, *Presence confirmation*): set by a
// guardian as `{ v: 1, salt, hash }` (scrypt N=2^15, r=8, p=1, 32-byte output,
// hex) inside a `guardianPin` field of a RELAYS event body, gift-wrapped to the
// ward. A pasted relay code carrying `guardianPin` is refused by the ward
// (docs/guardian-keys-open-questions.md, "How a guardian PIN reaches an
// Android ward"), so this app only ever sends it wrapped.
//
// The salt is 16 random bytes, lowercase hex; the scrypt input is the PIN's
// UTF-8 bytes (OQ "Guardian PIN salt and encoding"). The parameters come from
// the reference (`GUARDIAN_PIN_SCRYPT`) and the derivation is pinned by the
// golden vectors in `vectors/guardians/pin.json`.

import { scryptAsync } from "@noble/hashes/scrypt.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";
import type { EventTemplate } from "nostr-tools";
import { GUARDIAN_PIN_SCRYPT, validateGuardianPinRecord, type GuardianPinRecord } from "./sdk";
import { CHARTER_DEVICE_RELAYS, validateRelays } from "../wire/relayConfig";

/** Smallest PIN the app accepts. The ward stores only the hash. */
export const MIN_PIN_LENGTH = 6;

/** The scrypt hash of `pin` under `saltHex` (lowercase hex). */
export async function guardianPinHash(pin: string, saltHex: string): Promise<string> {
  const { N, r, p, dkLen } = GUARDIAN_PIN_SCRYPT;
  const out = await scryptAsync(new TextEncoder().encode(pin), hexToBytes(saltHex), { N, r, p, dkLen });
  return bytesToHex(out);
}

/**
 * Make a `guardianPin` record for `pin` with a fresh 16-byte salt (or the
 * given one, for tests). Refuses a PIN shorter than `MIN_PIN_LENGTH` digits
 * or one that is not all digits.
 */
export async function makeGuardianPinRecord(pin: string, saltHex?: string): Promise<GuardianPinRecord> {
  if (!/^[0-9]+$/.test(pin) || pin.length < MIN_PIN_LENGTH) {
    throw new Error(`Choose a PIN of at least ${MIN_PIN_LENGTH} digits.`);
  }
  const salt = saltHex ?? bytesToHex(crypto.getRandomValues(new Uint8Array(16)));
  const record = { v: 1 as const, salt, hash: await guardianPinHash(pin, salt) };
  const checked = validateGuardianPinRecord(record);
  if (!checked.ok) throw new Error("could not make a valid guardian PIN record");
  return checked.record;
}

/**
 * A RELAYS (31118) template carrying `guardianPin`: the ward's current relay
 * list, unchanged, plus the PIN record. Sign it with a key in the ward's
 * signer set and gift-wrap it (never paste it).
 */
export function relaysWithPinTemplate(
  machine: string,
  relays: string[],
  record: GuardianPinRecord,
  now: number,
): EventTemplate {
  if (!/^[0-9a-f]{64}$/.test(machine)) throw new Error("This device has no valid device code.");
  return {
    kind: CHARTER_DEVICE_RELAYS,
    created_at: now,
    tags: [["t", "charter-device"], ["p", machine]],
    content: JSON.stringify({ machine, relays: validateRelays(relays), guardianPin: record }),
  };
}
