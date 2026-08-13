// Offline-unlock UI logic — the pure, framework-free half of the guardian's
// offline-unlock flow (see `UnlockDevice.tsx`). Kept separate from the React
// component so the sanitising + code-derivation wiring is unit-testable without
// rendering. The crypto itself lives in `../unlock` and is NOT re-implemented
// here — this module only decides WHEN to call it and with what.

import type { SignerKind } from "../domain/types";
import { unlockCodeForDevice } from "../unlock";

/**
 * The challenge alphabet charterd shows on the lock screen: uppercase letters
 * and digits with the visually-ambiguous 0/1/I/O removed (this exact string is
 * the crypto-parity source of truth). A challenge the guardian reads back is
 * always exactly {@link UNLOCK_CHALLENGE_LEN} of these characters.
 */
export const UNLOCK_CHALLENGE_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export const UNLOCK_CHALLENGE_LEN = 4;

/**
 * Normalise raw challenge input to the on-device alphabet: uppercase it, drop
 * everything not in the alphabet (spaces, punctuation, the excluded 0/O/1/I/L),
 * and cap at {@link UNLOCK_CHALLENGE_LEN}. Idempotent — sanitising a sanitised
 * value is a no-op — so it's safe to use as a controlled-input value.
 */
export function sanitizeChallenge(raw: string): string {
  let out = "";
  for (const ch of raw.toUpperCase()) {
    if (UNLOCK_CHALLENGE_ALPHABET.includes(ch)) out += ch;
    if (out.length === UNLOCK_CHALLENGE_LEN) break;
  }
  return out;
}

/** True once a (sanitised) challenge is the full 4 characters. */
export function isChallengeComplete(challenge: string): boolean {
  return challenge.length === UNLOCK_CHALLENGE_LEN;
}

/** A device's machine key is usable only when it's 64 lowercase hex chars. */
export function isValidDevicePubkey(
  pubkey: string | null | undefined,
): pubkey is string {
  return typeof pubkey === "string" && /^[0-9a-f]{64}$/.test(pubkey);
}

/** Group the 8-digit code as "1234 5678" so it's easy to read out loud. */
export function formatUnlockCode(code: string): string {
  return /^\d{8}$/.test(code) ? `${code.slice(0, 4)} ${code.slice(4)}` : code;
}

/**
 * What the unlock screen should render, decided from the current inputs. Every
 * variant carries the sanitised `challenge` so the input can echo exactly what
 * was accepted. A code is only ever produced on the "code" variant, and ONLY
 * after the local-signer + valid-pubkey + complete-challenge gates all pass —
 * so a non-local signer or an unpaired device can never show a (bogus) code.
 */
export type UnlockView =
  /** The guardian key isn't held locally (a Signet/bunker signer). */
  | { kind: "not-local"; challenge: string }
  /** The device has no usable machine key (not fully paired). */
  | { kind: "no-pubkey"; challenge: string }
  /** Fewer than 4 valid characters entered so far. */
  | { kind: "incomplete"; challenge: string }
  /** Ready — `code` is the 8-digit answer to type into the device. */
  | { kind: "code"; challenge: string; code: string }
  /** Reading the guardian key failed (e.g. corrupt) — fail safe, no code. */
  | { kind: "error"; challenge: string };

/**
 * Decide the unlock view from the signer kind, the device's pubkey and the raw
 * challenge text. `loadGuardianSk` is invoked ONLY on the fully-gated code path
 * (local signer + valid pubkey + complete challenge) so the guardian key is
 * never touched for a non-local signer. Any throw from it (corrupt key) is
 * caught and surfaced as `error` rather than crashing the render.
 */
export function computeUnlockView(params: {
  signerKind: SignerKind;
  devicePubkey: string | null | undefined;
  rawChallenge: string;
  loadGuardianSk: () => Uint8Array;
}): UnlockView {
  const { signerKind, devicePubkey, rawChallenge, loadGuardianSk } = params;
  const challenge = sanitizeChallenge(rawChallenge);

  if (signerKind !== "local") return { kind: "not-local", challenge };
  if (!isValidDevicePubkey(devicePubkey)) return { kind: "no-pubkey", challenge };
  if (!isChallengeComplete(challenge)) return { kind: "incomplete", challenge };

  try {
    const code = unlockCodeForDevice(loadGuardianSk(), devicePubkey, challenge);
    return { kind: "code", challenge, code };
  } catch {
    return { kind: "error", challenge };
  }
}
