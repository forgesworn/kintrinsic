import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { isSealed, openSecret, sealSecret, vaultSupported } from "./keyVault";

// The guardian secret key lives on its OWN localStorage key — never bundled
// into the app-state blob.
//
// TWO generations, and the older one is a migration source, not a fallback:
//
//   v1  a plaintext bech32 `nsec…`. What every install before 2026-08-07 has.
//   v2  AES-GCM ciphertext under a NON-EXTRACTABLE key in IndexedDB — see
//       ./keyVault, and S4 in the security review for why plaintext at rest
//       for the family's root signing identity was worth removing.
//
// THE RULE THAT OVERRIDES EVERYTHING HERE: never lose the key. A guardian key
// that vanishes orphans every device the family ever paired — they each pinned
// the OLD pubkey and will simply stop accepting anything. So the migration
// never deletes v1 until a v2 blob has been written AND read back AND compared
// byte-for-byte; a vault that cannot be built leaves v1 exactly where it is and
// the app keeps working; and nothing in this module ever mints over a value it
// merely failed to read.
const KEY_V1 = "charter.guardian.key.v1";
const KEY_V2 = "charter.guardian.key.v2";

/**
 * The decrypted key for this session.
 *
 * Opening the vault is asynchronous and essentially every caller here is
 * synchronous — the signer, the relay poll, the QR screen. Rather than turn
 * the whole app async for a security property it would not gain (the page has
 * to hold a usable key in memory to sign at all), `initGuardianKey()` opens it
 * once before the first render and everything else reads this.
 */
let cached: Uint8Array | null = null;
/** Whether `initGuardianKey` has run — see `readGuardianKey`'s fail-closed. */
let initialised = false;
/** A stored key that is present and could NOT be opened. Latched at init. */
let unreadable = false;

/**
 * The outcome of reading the persisted guardian key. We DISTINGUISH a fresh
 * install (`absent` — nothing stored, safe to mint) from a present-but-
 * undecodable value (`corrupt`). Conflating the two is a key-loss trap: minting
 * over a corrupt value silently swaps in a new, unpaired identity and orphans
 * every device this guardian paired (they pinned the OLD pubkey) — the exact
 * cliff `keyBackup.ts` exists to avoid. `corrupt` must route the parent to
 * restore-from-backup, never to a silent overwrite.
 */
export type GuardianKeyLoad =
  | { kind: "ok"; key: Uint8Array }
  | { kind: "absent" }
  | { kind: "corrupt" };

/** Thrown when a caller demands THE guardian key but the stored value is
 *  present yet undecodable — so the caller can offer restore instead of
 *  silently minting a fresh, unpaired identity over it. */
export class CorruptGuardianKeyError extends Error {
  constructor() {
    super("the stored guardian key could not be decoded");
    this.name = "CorruptGuardianKeyError";
  }
}

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
    // Storage unavailable (private mode / quota). Nothing to do here: the key
    // still works for this session out of `cached`. Failing loudly would help
    // nobody in the middle of a pairing.
  }
}

function removeItem(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* see setItem */
  }
}

/** Decode a stored v1 `nsec…`, or null if it is not one. */
function decodeV1(raw: string): Uint8Array | null {
  try {
    const decoded = nip19.decode(raw);
    if (decoded.type === "nsec") return decoded.data;
  } catch {
    /* fall through — a present value we cannot decode is CORRUPT, not absent */
  }
  return null;
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

/**
 * Move a key that is already in hand into the vault, and only then drop the
 * plaintext. Returns true when v1 was actually retired.
 *
 * The read-back-and-compare is the whole point. Sealing can appear to succeed
 * while the wrapping key failed to persist, or while localStorage silently
 * dropped the write; deleting v1 on the strength of an unverified write is how
 * a security improvement turns into the exact catastrophe it was guarding
 * against.
 */
async function sealAndRetireV1(secret: Uint8Array): Promise<boolean> {
  if (!vaultSupported()) return false;
  try {
    const blob = await sealSecret(secret);
    setItem(KEY_V2, blob);
    const readBack = getItem(KEY_V2);
    const ok =
      !!readBack && isSealed(readBack) && sameBytes(await openSecret(readBack), secret);
    if (!ok) {
      // TAKE THE BAD BLOB BACK OUT. Leaving it was a landmine: the next boot
      // reads v2 first, fails to open it, and reports the key corrupt — while
      // the perfectly good plaintext sits untouched two lines away in the same
      // storage. A half-written vault must leave NO trace, or the safety net
      // becomes the thing that drops you.
      removeItem(KEY_V2);
      return false;
    }
    removeItem(KEY_V1);
    return true;
  } catch {
    // No vault today (a WebView without IndexedDB, storage denied, a locked
    // profile). v1 stays; the app is unchanged from how it has always worked.
    // Same reasoning as above: whatever we may have written, take it back.
    removeItem(KEY_V2);
    return false;
  }
}

/**
 * Open the vault (or migrate into it) BEFORE the app renders. Idempotent.
 *
 * Every synchronous reader below depends on this having run. `main.tsx` awaits
 * it, and it is written so that no outcome — no vault, no storage, a corrupt
 * blob — can throw: a guardian must never be met by a blank screen because
 * their browser dislikes IndexedDB.
 */
export async function initGuardianKey(): Promise<void> {
  if (initialised) return;
  const sealed = getItem(KEY_V2);
  const legacy = getItem(KEY_V1);

  if (sealed) {
    try {
      cached = await openSecret(sealed);
      initialised = true;
      // Both present means a previous migration didn't finish. The vault opens,
      // so finish it now rather than leaving the plaintext lying about.
      if (legacy && cached) removeItem(KEY_V1);
      return;
    } catch {
      // The vault will not open. Before calling this key lost, LOOK FOR THE
      // PLAINTEXT — `sealAndRetireV1` only removes v1 after a verified
      // round-trip, so if v1 is still here the sealed copy is the thing that
      // is wrong, not the key. Falling straight to `corrupt` here would send a
      // guardian to restore-from-backup while their working key sat in the
      // same storage, which is the precise catastrophe this module exists to
      // prevent.
      if (!legacy) {
        unreadable = true;
        initialised = true;
        return;
      }
    }
  }

  if (legacy) {
    const decoded = decodeV1(legacy);
    if (decoded) {
      cached = decoded;
      // Fire it into the vault now that we hold it. Awaited (not
      // fire-and-forget) so the plaintext is gone before the first render can
      // ship a stack trace or a crash report anywhere. A failure here is
      // survivable and self-healing: v1 stays, v2 is cleaned up, and the next
      // load tries again.
      await sealAndRetireV1(decoded);
    } else {
      unreadable = true;
    }
  }
  initialised = true;
}

/**
 * Read the persisted guardian key WITHOUT ever minting. Returns a discriminated
 * result so callers can tell "no key yet" (`absent`) from "present but broken"
 * (`corrupt`) — the basis of the no-silent-overwrite guarantee.
 */
export function readGuardianKey(): GuardianKeyLoad {
  if (cached) return { kind: "ok", key: cached };
  if (unreadable) return { kind: "corrupt" };

  // No cache ⇒ ask STORAGE, every time. Never "we have already initialised, so
  // there must be nothing": storage can gain a key after init — a restore in
  // another tab, a clear followed by a write — and answering `absent` over a
  // key that is really there is an invitation to mint straight over it.
  //
  // Synchronous readers only, and FAIL CLOSED on the sealed blob: if a v2 key
  // exists that this module has not opened, the honest answer is "present, and
  // I cannot read it", not "there isn't one".
  const legacy = getItem(KEY_V1);
  if (legacy) {
    const decoded = decodeV1(legacy);
    return decoded ? { kind: "ok", key: decoded } : { kind: "corrupt" };
  }
  return getItem(KEY_V2) ? { kind: "corrupt" } : { kind: "absent" };
}

/**
 * Load the persisted guardian secret key, generating + persisting one ONLY on a
 * true fresh install (nothing stored). If a value IS stored but can't be decoded
 * this throws [`CorruptGuardianKeyError`] rather than minting over it — the app
 * surfaces that as "restore from backup" so paired devices aren't orphaned.
 */
export function loadOrCreateGuardianKey(): Uint8Array {
  const loaded = readGuardianKey();
  if (loaded.kind === "ok") return loaded.key;
  if (loaded.kind === "corrupt") throw new CorruptGuardianKeyError();
  const sk = generateSecretKey();
  cached = sk;
  initialised = true;
  // Persist the plaintext FIRST, then seal and retire it. Backwards-looking,
  // but a brand-new key that exists only in a promise is a key that a closed
  // tab destroys — and it is the one key with no backup yet by definition.
  // The plaintext window is one turn of the event loop; losing the identity
  // is forever.
  setItem(KEY_V1, nip19.nsecEncode(sk));
  void sealAndRetireV1(sk);
  return sk;
}

/** The guardian pubkey (hex) for the persisted key, creating it if needed.
 *  Throws [`CorruptGuardianKeyError`] if a stored key is present but undecodable
 *  — callers on the render path must gate on {@link readGuardianKey} first. */
export function guardianPubkeyHex(): string {
  return getPublicKey(loadOrCreateGuardianKey());
}

/** Forget the guardian key — used on deliberate re-pair / identity reset. */
export function clearGuardianKey(): void {
  cached = null;
  unreadable = false;
  initialised = true;
  removeItem(KEY_V1);
  removeItem(KEY_V2);
}

/**
 * Install a restored 32-byte guardian secret as THE guardian key (recovery on
 * a new device). Overwrites any existing key — the caller confirms first. The
 * restored pubkey matches every device this guardian already paired, so those
 * pairings revive with no re-pairing.
 */
export function importGuardianKey(secret: Uint8Array): void {
  if (secret.length !== 32) throw new Error("guardian secret must be 32 bytes");
  cached = secret;
  unreadable = false;
  initialised = true;
  // Same order and same reason as minting: land it durably, then seal.
  setItem(KEY_V1, nip19.nsecEncode(secret));
  removeItem(KEY_V2);
  void sealAndRetireV1(secret);
}

/** Test seam — forget what this module cached, as a page reload would. */
export function resetGuardianKeyCacheForTests(): void {
  cached = null;
  initialised = false;
  unreadable = false;
}
