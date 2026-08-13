// Offline guardian unlock — the TS mirror of
// `core/crates/charter-crypto/src/unlock.rs`. It MUST produce identical codes;
// the shared parity vectors are asserted in both `unlock.test.ts` and the Rust
// `unlock` test module. See the Rust doc comment for the full rationale.
//
// Construction:
//   secret = HMAC-SHA256(conv_key, "charter-offline-unlock-v1")   // domain sep
//   mac    = HMAC-SHA256(secret, challenge_ascii_bytes)
//   code   = HOTP (RFC 4226) dynamic truncation of `mac`, mod 10^8, 8 digits

import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { nip44 } from "nostr-tools";

const DOMAIN = new TextEncoder().encode("charter-offline-unlock-v1");
export const UNLOCK_CODE_DIGITS = 8;

function hmacSha256(key: Uint8Array, msg: Uint8Array): Uint8Array {
  return hmac(sha256, key, msg);
}

/**
 * The unlock code for `challenge`, derived from the guardian↔machine shared
 * secret `convKey` (the NIP-44 conversation key). Returns exactly
 * `UNLOCK_CODE_DIGITS` ASCII digits — identical to the Rust `unlock_code`.
 */
export function unlockCode(convKey: Uint8Array, challenge: string): string {
  const secret = hmacSha256(convKey, DOMAIN);
  const mac = hmacSha256(secret, new TextEncoder().encode(challenge));
  // RFC 4226 (HOTP) dynamic truncation. `>>> 0` coerces to unsigned so the
  // result matches Rust's u32 arithmetic exactly.
  const off = mac[mac.length - 1] & 0x0f;
  const bin =
    (((mac[off] & 0x7f) << 24) |
      (mac[off + 1] << 16) |
      (mac[off + 2] << 8) |
      mac[off + 3]) >>>
    0;
  const code = bin % 100_000_000;
  return code.toString().padStart(UNLOCK_CODE_DIGITS, "0");
}

/**
 * The unlock code for a paired device whose pubkey (hex) the guardian holds the
 * key for. Derives the SAME NIP-44 conversation key charterd uses on the device
 * side (symmetric ECDH: guardianSk+devicePk == machineSk+guardianPk).
 */
export function unlockCodeForDevice(
  guardianSk: Uint8Array,
  devicePubkeyHex: string,
  challenge: string,
): string {
  const convKey = nip44.getConversationKey(guardianSk, devicePubkeyHex);
  return unlockCode(convKey, challenge);
}
