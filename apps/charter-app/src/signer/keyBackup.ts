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

export interface GuardianBackup {
  secret: Uint8Array;
  /** The persisted app-state JSON (children/devices/policies), when the blob carries it. */
  stateJson?: string;
}

/**
 * Encrypt the guardian secret PLUS the household state under `passphrase`
 * (v2 blob). `stateJson` may be null for a key-only backup.
 */
export async function encryptGuardianBackup(
  secret: Uint8Array,
  stateJson: string | null,
  passphrase: string,
): Promise<string> {
  if (secret.length !== 32) {
    throw new KeyBackupError("guardian secret must be 32 bytes");
  }
  if (passphrase.length < 8) {
    throw new KeyBackupError("passphrase must be at least 8 characters");
  }
  const payload = new TextEncoder().encode(
    JSON.stringify({ secretHex: bytesToHex(secret), state: stateJson ?? undefined }),
  );
  const salt = crypto.getRandomValues(new Uint8Array(SALT_BYTES));
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const key = await deriveAesKey(passphrase, salt);
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: buf(iv) }, key, buf(payload)),
  );
  return [MAGIC, VERSION2, b64(salt), b64(iv), b64(ct)].join(".");
}

/**
 * Decrypt EITHER blob generation: v1 (key-only) or v2 (key + household).
 * The one restore entry point from here on.
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
  if (parts[1] !== String(VERSION2)) {
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
  let parsed: { secretHex?: string; state?: string };
  try {
    parsed = JSON.parse(new TextDecoder().decode(pt));
  } catch {
    throw new KeyBackupError("the backup did not contain a valid payload");
  }
  if (typeof parsed.secretHex !== "string" || !/^[0-9a-f]{64}$/.test(parsed.secretHex)) {
    throw new KeyBackupError("the backup did not contain a valid key");
  }
  const out: GuardianBackup = { secret: hexToBytes(parsed.secretHex) };
  if (typeof parsed.state === "string" && parsed.state.length > 0) out.stateJson = parsed.state;
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
  if (backup.stateJson && hasHousehold(localStateJson)) return { secret: backup.secret };
  return backup;
}
