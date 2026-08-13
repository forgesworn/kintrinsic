// At-rest custody for the guardian secret key (S4, review 2026-08-07).
//
// The key used to sit in localStorage as a plaintext `nsec…`. That is one
// `localStorage.getItem` away from total guardian-identity theft — forge any
// policy, approve any request, release every device in the family — and it
// sits there whether the app is open or not, readable by anything that ever
// gets to run a line of script on this origin, and copyable wholesale by
// anything that snapshots browser storage.
//
// WHAT THIS BUYS, precisely, so nobody over-trusts it:
//
//   ✓ No plaintext key at rest. What is stored is AES-GCM ciphertext.
//   ✓ The wrapping key is a NON-EXTRACTABLE CryptoKey living in IndexedDB.
//     The browser will hand it to `crypto.subtle.decrypt` and to nothing
//     else — it cannot be read out, serialised, or carried off the device.
//     A copied storage profile is inert without the origin's own IndexedDB.
//   ✗ It does NOT defeat script that is already running on this origin after
//     the app has booted: the app must be able to sign, so a decrypted copy
//     lives in memory. In a browser, that is unavoidable for any key the page
//     itself uses.
//
// The compensating control for the case this does not cover is the CSP in
// index.html (S5). These two ship together and neither is sufficient alone.

const DB_NAME = "charter-key-vault";
const DB_VERSION = 1;
const STORE = "wrap";
const WRAP_ID = "guardian-v1";

/** Sealed-blob prefix, so a stored value announces its own format. */
const SEAL_PREFIX = "v2";

function idbOpen(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      if (!req.result.objectStoreNames.contains(STORE)) req.result.createObjectStore(STORE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("indexedDB open failed"));
    req.onblocked = () => reject(new Error("indexedDB blocked"));
  });
}

function idbGet(db: IDBDatabase, key: string): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const req = db.transaction(STORE, "readonly").objectStore(STORE).get(key);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("indexedDB read failed"));
  });
}

function idbPut(db: IDBDatabase, key: string, value: unknown): Promise<void> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, "readwrite");
    tx.objectStore(STORE).put(value, key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("indexedDB write failed"));
    tx.onabort = () => reject(tx.error ?? new Error("indexedDB write aborted"));
  });
}

/** True when this environment can actually hold a vault. */
export function vaultSupported(): boolean {
  return (
    typeof indexedDB !== "undefined" &&
    typeof crypto !== "undefined" &&
    typeof crypto.subtle?.encrypt === "function"
  );
}

/**
 * The wrapping key: AES-GCM 256, **`extractable: false`**, created once and
 * kept in IndexedDB. Structured-clone stores the CryptoKey handle itself, so
 * the raw bytes never exist anywhere JavaScript can reach them — not in this
 * module, not in a backup of localStorage, not in a copied profile.
 */
async function wrapKey(): Promise<CryptoKey> {
  const db = await idbOpen();
  try {
    const existing = await idbGet(db, WRAP_ID);
    if (existing instanceof CryptoKey) return existing;
    const fresh = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, [
      "encrypt",
      "decrypt",
    ]);
    await idbPut(db, WRAP_ID, fresh);
    // Read back rather than trusting the write: a vault whose key did not
    // actually persist would seal a secret today and lose it at the next
    // reload, which is the one failure this whole file must not cause.
    const confirmed = await idbGet(db, WRAP_ID);
    if (!(confirmed instanceof CryptoKey)) throw new Error("wrap key did not persist");
    return confirmed;
  } finally {
    db.close();
  }
}

/** A plain-ArrayBuffer copy — WebCrypto's types reject the SharedArrayBuffer
 *  case that a bare `Uint8Array` leaves open, and callers hand us views from
 *  everywhere. */
function toBuffer(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.length);
  new Uint8Array(out).set(bytes);
  return out;
}

function toB64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

function fromB64(s: string): Uint8Array<ArrayBuffer> {
  const bin = atob(s);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Seal 32 secret bytes into the storable blob `v2:<iv>:<ciphertext>`. */
export async function sealSecret(secret: Uint8Array): Promise<string> {
  const key = await wrapKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, toBuffer(secret)),
  );
  return `${SEAL_PREFIX}:${toB64(iv)}:${toB64(ct)}`;
}

/** True for a blob this module wrote — cheap enough to call before opening. */
export function isSealed(blob: string): boolean {
  return blob.startsWith(`${SEAL_PREFIX}:`);
}

/**
 * Open a sealed blob. Throws on anything that is not a clean round-trip —
 * wrong format, missing vault key, failed GCM tag, wrong length. Callers must
 * treat a throw as "present but unreadable", NEVER as "absent": minting over
 * an unreadable key silently orphans every device this guardian ever paired.
 */
export async function openSecret(blob: string): Promise<Uint8Array> {
  const parts = blob.split(":");
  if (parts.length !== 3 || parts[0] !== SEAL_PREFIX) throw new Error("not a sealed guardian key");
  const iv = fromB64(parts[1]);
  const ct = fromB64(parts[2]);
  const key = await wrapKey();
  const plain = new Uint8Array(
    await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ct),
  );
  if (plain.length !== 32) throw new Error("sealed guardian key is the wrong length");
  return plain;
}
