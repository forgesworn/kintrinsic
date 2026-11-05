// Passphrase-encrypted backup of the guardian key — the answer to the single
// scariest cliff in the system: the guardian secret lives in this browser's
// localStorage, so losing the phone (or clearing site data) would orphan EVERY
// paired device, forcing a re-pair of each one. Because pairing pins the
// guardian PUBKEY on each device, restoring the SAME secret on a new phone
// revives every existing pairing with zero re-pairing — the backup is the
// whole recovery story.
//
// Crypto: PBKDF2-HMAC-SHA256 (600k iterations, OWASP 2023) stretches the
// parent's passphrase into an AES-256-GCM key; the 32-byte scalar is sealed
// under a random 96-bit IV with a random 128-bit salt. All WebCrypto — no
// dependency, no key material ever leaves the device except as ciphertext the
// parent chose to export. The blob is self-describing (versioned) and ASCII so
// it copy/pastes, saves as a .txt, or rides a QR.

import { getPublicKey } from "nostr-tools";

const MAGIC = "CHARTER-KEYBAK";
const VERSION = 1;
/** v2 payload = JSON {secretHex, state?} — the key PLUS the whole household
 *  (children, devices, policies), so one paste brings a fresh install fully
 *  up to speed (decented, 2026-07-23). v1 (key-only) blobs restore forever. */
const VERSION2 = 2;
/** v3 payload = JSON {state, workingKeys} with NO key: a family whose identity
 *  is in My Signet (decision D5). The only local key there is this phone's
 *  working key, and a working key is never copied. */
const VERSION3 = 3;
const PBKDF2_ITERATIONS = 600_000;
const SALT_BYTES = 16;
const IV_BYTES = 12;

export class KeyBackupError extends Error {}

function b64(bytes: Uint8Array): string {
  let s = "";
  for (const byte of bytes) s += String.fromCharCode(byte);
  return btoa(s);
}

function unb64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** A standalone ArrayBuffer copy — satisfies WebCrypto's BufferSource typing
 *  regardless of the source view's (possibly shared) backing buffer. */
function buf(bytes: Uint8Array): ArrayBuffer {
  return bytes.slice().buffer as ArrayBuffer;
}

async function deriveAesKey(
  passphrase: string,
  salt: Uint8Array,
): Promise<CryptoKey> {
  const material = await crypto.subtle.importKey(
    "raw",
    buf(new TextEncoder().encode(passphrase)),
    "PBKDF2",
    false,
    ["deriveKey"],
  );
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", salt: buf(salt), iterations: PBKDF2_ITERATIONS, hash: "SHA-256" },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

/**
 * Encrypt a 32-byte guardian secret under `passphrase`. Returns a portable
 * ASCII blob: `CHARTER-KEYBAK.1.<saltB64>.<ivB64>.<ctB64>`.
 */
export async function encryptGuardianKey(
  secret: Uint8Array,
  passphrase: string,
): Promise<string> {
  if (secret.length !== 32) {
    throw new KeyBackupError("guardian secret must be 32 bytes");
  }
  if (passphrase.length < 8) {
    throw new KeyBackupError("passphrase must be at least 8 characters");
  }
  const salt = crypto.getRandomValues(new Uint8Array(SALT_BYTES));
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const key = await deriveAesKey(passphrase, salt);
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: buf(iv) }, key, buf(secret)),
  );
  return [MAGIC, VERSION, b64(salt), b64(iv), b64(ct)].join(".");
}

/**
 * Decrypt a backup blob with `passphrase`, returning the 32-byte secret. Throws
 * [`KeyBackupError`] on a malformed blob or a wrong passphrase (GCM auth fails)
 * — a wrong passphrase is indistinguishable from tampering, by design.
 */
export async function decryptGuardianKey(
  blob: string,
  passphrase: string,
): Promise<Uint8Array> {
  const parts = blob.trim().split(".");
  if (parts.length !== 5 || parts[0] !== MAGIC) {
    throw new KeyBackupError("that isn't a Kintrinsic key backup");
  }
  if (parts[1] !== String(VERSION)) {
    throw new KeyBackupError(`unsupported backup version ${parts[1]}`);
  }
  let salt: Uint8Array;
  let iv: Uint8Array;
  let ct: Uint8Array;
  try {
    salt = unb64(parts[2]);
    iv = unb64(parts[3]);
    ct = unb64(parts[4]);
  } catch {
    throw new KeyBackupError("the backup is corrupted");
  }
  const key = await deriveAesKey(passphrase, salt);
  let pt: ArrayBuffer;
  try {
    pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: buf(iv) }, key, buf(ct));
  } catch {
    throw new KeyBackupError("wrong passphrase, or the backup is corrupted");
  }
  const secret = new Uint8Array(pt);
  if (secret.length !== 32) {
    throw new KeyBackupError("the backup did not contain a valid key");
  }
  return secret;
}

/**
 * The guardian fingerprint a backup will restore to (hex pubkey), WITHOUT
 * installing it — so the restore UI can show "this brings back guardian
 * 2ba6d49b…" and the parent can confirm it's the right identity before it
 * replaces anything.
 */
export async function backupFingerprint(
  blob: string,
  passphrase: string,
): Promise<string> {
  return getPublicKey(await decryptGuardianKey(blob, passphrase));
}

function bytesToHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

function hexToBytes(h: string): Uint8Array {
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** A working key of this identity, by its PUBLIC key only: what a restore may offer to withdraw. */
export interface BackedUpWorkingKey {
  pubkey: string;
  label?: string;
}

export interface GuardianBackup {
  /** The guardian identity's key. Absent from a v3 blob: the identity is in My Signet. */
  secret?: Uint8Array;
  /** The persisted app-state JSON (children/devices/policies), when the blob carries it. */
  stateJson?: string;
  /** This identity's working keys when the backup was made (public keys and labels, never secrets). */
  workingKeys?: BackedUpWorkingKey[];
}

/**
 * Encrypt the household state under `passphrase`, with the guardian secret
 * (v2 blob) or, for an identity held in My Signet, without one (v3). The
 * secret is only ever the identity's own key, never a working key. `stateJson`
 * may be null for a key-only backup. `workingKeys` lists this identity's
 * working keys, public keys only.
 */
export async function encryptGuardianBackup(
  secret: Uint8Array | null,
  stateJson: string | null,
  passphrase: string,
  workingKeys: readonly BackedUpWorkingKey[] = [],
): Promise<string> {
  if (secret !== null && secret.length !== 32) {
    throw new KeyBackupError("guardian secret must be 32 bytes");
  }
  if (secret === null && !stateJson) {
    throw new KeyBackupError("a backup with no key must carry the family");
  }
  if (passphrase.length < 8) {
    throw new KeyBackupError("passphrase must be at least 8 characters");
  }
  const keys = workingKeys.filter((k) => /^[0-9a-f]{64}$/.test(k.pubkey)).map((k) => ({ pubkey: k.pubkey, ...(k.label ? { label: k.label } : {}) }));
  const payload = new TextEncoder().encode(
    JSON.stringify({
      ...(secret !== null ? { secretHex: bytesToHex(secret) } : {}),
      state: stateJson ?? undefined,
      ...(keys.length > 0 ? { workingKeys: keys } : {}),
    }),
  );
  const salt = crypto.getRandomValues(new Uint8Array(SALT_BYTES));
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const key = await deriveAesKey(passphrase, salt);
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: buf(iv) }, key, buf(payload)),
  );
  return [MAGIC, secret !== null ? VERSION2 : VERSION3, b64(salt), b64(iv), b64(ct)].join(".");
}

/**
 * Decrypt any blob generation: v1 (key-only), v2 (key + household) or v3
 * (household only, an identity in My Signet). The one restore entry point
 * from here on.
 */
export async function decryptGuardianBackup(
  blob: string,
  passphrase: string,
): Promise<GuardianBackup> {
  const parts = blob.trim().split(".");
  if (parts.length !== 5 || parts[0] !== MAGIC) {
    throw new KeyBackupError("that isn't a Kintrinsic key backup");
  }
  if (parts[1] === String(VERSION)) {
    return { secret: await decryptGuardianKey(blob, passphrase) };
  }
  if (parts[1] !== String(VERSION2) && parts[1] !== String(VERSION3)) {
    throw new KeyBackupError(`unsupported backup version ${parts[1]}`);
  }
  const keyless = parts[1] === String(VERSION3);
  let salt: Uint8Array;
  let iv: Uint8Array;
  let ct: Uint8Array;
  try {
    salt = unb64(parts[2]);
    iv = unb64(parts[3]);
    ct = unb64(parts[4]);
  } catch {
    throw new KeyBackupError("the backup is corrupted");
  }
  const key = await deriveAesKey(passphrase, salt);
  let pt: ArrayBuffer;
  try {
    pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: buf(iv) }, key, buf(ct));
  } catch {
    throw new KeyBackupError("wrong passphrase, or the backup is corrupted");
  }
  let parsed: { secretHex?: string; state?: string; workingKeys?: unknown };
  try {
    parsed = JSON.parse(new TextDecoder().decode(pt));
  } catch {
    throw new KeyBackupError("the backup did not contain a valid payload");
  }
  const out: GuardianBackup = {};
  if (!keyless) {
    if (typeof parsed.secretHex !== "string" || !/^[0-9a-f]{64}$/.test(parsed.secretHex)) {
      throw new KeyBackupError("the backup did not contain a valid key");
    }
    out.secret = hexToBytes(parsed.secretHex);
  }
  if (typeof parsed.state === "string" && parsed.state.length > 0) out.stateJson = parsed.state;
  if (keyless && !out.stateJson) throw new KeyBackupError("the backup did not contain your family");
  if (Array.isArray(parsed.workingKeys)) {
    const keys = parsed.workingKeys.flatMap((k): BackedUpWorkingKey[] => {
      if (typeof k !== "object" || k === null) return [];
      const o = k as { pubkey?: unknown; label?: unknown };
      if (typeof o.pubkey !== "string" || !/^[0-9a-f]{64}$/.test(o.pubkey)) return [];
      return [{ pubkey: o.pubkey, ...(typeof o.label === "string" && o.label ? { label: o.label.slice(0, 64) } : {}) }];
    });
    if (keys.length > 0) out.workingKeys = keys;
  }
  return out;
}

/** True when `stateJson` (the persisted store) holds at least one child. */
function hasHousehold(stateJson: string | null): boolean {
  if (!stateJson) return false;
  try {
    const parsed = JSON.parse(stateJson) as { children?: unknown };
    return Array.isArray(parsed.children) && parsed.children.length > 0;
  } catch {
    return false;
  }
}

/**
 * The corrupt-key recovery screen's restore: the key always comes back; the
 * backup's household only when this phone has none of its own. There the key
 * is unreadable but the family data is usually intact — and NEWER than any
 * backup — so it must not be rolled back; on a wiped or new phone there is
 * nothing to lose and the v2 household is the whole point of the blob.
 */
export async function recoverGuardianBackup(
  blob: string,
  passphrase: string,
  localStateJson: string | null,
): Promise<GuardianBackup> {
  const backup = await decryptGuardianBackup(blob, passphrase);
  // A keyless backup is all household: there is nothing else to bring back.
  if (backup.secret && backup.stateJson && hasHousehold(localStateJson)) {
    return { secret: backup.secret, ...(backup.workingKeys ? { workingKeys: backup.workingKeys } : {}) };
  }
  return backup;
}
