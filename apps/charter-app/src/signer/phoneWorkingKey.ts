// The separate working key W of a family whose guardian identity R is this
// phone's own key (decision D5, docs/guardian-keys-open-questions.md "Moving
// a phone-held root"). R stays in the guardian key slot (`./guardianKey`) and
// keeps signing the rules; W is minted here on first need and approved by R,
// and then confirms in person, seals what R signs and objects on its own.
//
// It lives on its own localStorage key, sealed like the guardian key when the
// vault works, and is never put in a backup: a working key is generated where
// it is used and never copied (contract, *Guardian roots and working keys*).
// Losing it costs nothing that can't be redone: R mints and approves another.
//
// Once a move to My Signet has finished and the guardian chose to delete R,
// W is promoted into the guardian key slot and this slot is cleared, so the
// phone is then exactly an install with a My Signet identity.

import { generateSecretKey, nip19 } from "nostr-tools";
import { isSealed, openSecret, sealSecret, vaultSupported } from "./keyVault";

const SLOT_V1 = "charter.guardian.working.v1";
const SLOT_V2 = "charter.guardian.working.v2";

let cached: Uint8Array | null = null;

function getItem(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function setItem(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Storage unavailable: the key still works for this session.
  }
}

function removeItem(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* see setItem */
  }
}

function decodeV1(raw: string): Uint8Array | null {
  try {
    const d = nip19.decode(raw);
    return d.type === "nsec" ? d.data : null;
  } catch {
    return null;
  }
}

/** Seal the plaintext copy into the vault, and drop the plaintext only after a verified read-back. */
async function seal(secret: Uint8Array): Promise<void> {
  if (!vaultSupported()) return;
  try {
    const blob = await sealSecret(secret);
    setItem(SLOT_V2, blob);
    const back = getItem(SLOT_V2);
    const opened = back && isSealed(back) ? await openSecret(back) : null;
    if (!opened || opened.length !== secret.length || opened.some((b, i) => b !== secret[i])) {
      removeItem(SLOT_V2);
      return;
    }
    removeItem(SLOT_V1);
  } catch {
    removeItem(SLOT_V2);
  }
}

/** Open the slot before the first render (beside `initGuardianKey`). Never throws. */
export async function initPhoneWorkingKey(): Promise<void> {
  const sealed = getItem(SLOT_V2);
  const legacy = getItem(SLOT_V1);
  if (sealed) {
    try {
      cached = await openSecret(sealed);
      if (legacy) removeItem(SLOT_V1);
      return;
    } catch {
      // Unreadable: fall through to the plaintext copy, if any. With neither,
      // the slot reads as empty and R mints a fresh W when one is needed.
    }
  }
  if (legacy) {
    const d = decodeV1(legacy);
    if (d) {
      cached = d;
      await seal(d);
    }
  }
}

/** W's secret, or null when this phone has none (a My Signet install, or a phone-held root that hasn't needed one). */
export function readPhoneWorkingKey(): Uint8Array | null {
  if (cached) return cached;
  const legacy = getItem(SLOT_V1);
  if (legacy) {
    const d = decodeV1(legacy);
    if (d) cached = d;
  }
  return cached;
}

/** Generate and keep a fresh W. Replaces any W already in the slot. */
export function mintPhoneWorkingKey(): Uint8Array {
  const sk = generateSecretKey();
  cached = sk;
  setItem(SLOT_V1, nip19.nsecEncode(sk));
  removeItem(SLOT_V2);
  void seal(sk);
  return sk;
}

/** Forget W (after it was promoted into the guardian key slot). */
export function clearPhoneWorkingKey(): void {
  cached = null;
  removeItem(SLOT_V1);
  removeItem(SLOT_V2);
}

/** Test seam: forget the cached key, as a reload would. */
export function resetPhoneWorkingKeyForTests(): void {
  cached = null;
}
