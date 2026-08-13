// The guardian-key vault and, more importantly, the MIGRATION into it (S4).
//
// The migration is where the danger lives. Losing a guardian key is not a
// degraded experience, it is the end of the family's install: every paired
// device pinned the old pubkey and will simply stop accepting anything, with
// no way back except a backup the parent may never have made. So these tests
// spend most of their attention on the failure paths — no vault, a wrapping
// key that does not persist, a blob that will not open — and assert the same
// thing about each: the key is still there afterwards.

import "fake-indexeddb/auto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getPublicKey, nip19 } from "nostr-tools";
import { isSealed, openSecret, sealSecret, vaultSupported } from "./keyVault";
import {
  clearGuardianKey,
  importGuardianKey,
  initGuardianKey,
  loadOrCreateGuardianKey,
  readGuardianKey,
  resetGuardianKeyCacheForTests,
} from "./guardianKey";

const KEY_V1 = "charter.guardian.key.v1";
const KEY_V2 = "charter.guardian.key.v2";
const SECRET = new Uint8Array(32).fill(7);

/** Wipe every trace, the way a brand-new browser profile would present. */
async function freshProfile() {
  resetGuardianKeyCacheForTests();
  localStorage.clear();
  await new Promise<void>((resolve) => {
    const req = indexedDB.deleteDatabase("charter-key-vault");
    req.onsuccess = req.onerror = req.onblocked = () => resolve();
  });
}

/** Reload the page: storage survives, the module's memory does not. */
function reload() {
  resetGuardianKeyCacheForTests();
}

beforeEach(freshProfile);
afterEach(() => vi.restoreAllMocks());

describe("keyVault", () => {
  it("is supported once IndexedDB and WebCrypto are present", () => {
    expect(vaultSupported()).toBe(true);
  });

  it("round-trips a secret", async () => {
    const blob = await sealSecret(SECRET);
    expect(isSealed(blob)).toBe(true);
    expect(Array.from(await openSecret(blob))).toEqual(Array.from(SECRET));
  });

  it("never puts the secret in the blob", async () => {
    const blob = await sealSecret(SECRET);
    const hex = Array.from(SECRET, (b) => b.toString(16).padStart(2, "0")).join("");
    expect(blob).not.toContain(hex);
    expect(blob).not.toContain(nip19.nsecEncode(SECRET));
  });

  it("uses a fresh nonce each time, so the same key seals differently", async () => {
    expect(await sealSecret(SECRET)).not.toBe(await sealSecret(SECRET));
  });

  // The property the whole design rests on: the wrapping key cannot be read
  // out of the browser, so a copied localStorage is inert without it.
  it("keeps the wrapping key non-extractable", async () => {
    await sealSecret(SECRET);
    const db = await new Promise<IDBDatabase>((res, rej) => {
      const r = indexedDB.open("charter-key-vault", 1);
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
    const stored = await new Promise<unknown>((res, rej) => {
      const r = db.transaction("wrap", "readonly").objectStore("wrap").get("guardian-v1");
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
    db.close();
    expect(stored).toBeInstanceOf(CryptoKey);
    expect((stored as CryptoKey).extractable).toBe(false);
    await expect(crypto.subtle.exportKey("raw", stored as CryptoKey)).rejects.toThrow();
  });

  it("refuses a tampered blob rather than returning garbage", async () => {
    const blob = await sealSecret(SECRET);
    const parts = blob.split(":");
    // Flip a base64 character in the ciphertext — the GCM tag must catch it.
    const ct = parts[2];
    const flipped = (ct[0] === "A" ? "B" : "A") + ct.slice(1);
    await expect(openSecret(`${parts[0]}:${parts[1]}:${flipped}`)).rejects.toThrow();
  });

  it("refuses anything that isn't one of ours", async () => {
    for (const bad of ["", "v1:x:y", nip19.nsecEncode(SECRET), "v2:only-two-parts"]) {
      await expect(openSecret(bad)).rejects.toThrow();
    }
  });
});

describe("migrating a plaintext key into the vault", () => {
  it("seals an existing v1 key and removes the plaintext", async () => {
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    await initGuardianKey();

    expect(localStorage.getItem(KEY_V1), "the plaintext must be gone").toBeNull();
    const sealed = localStorage.getItem(KEY_V2);
    expect(sealed).toBeTruthy();
    expect(isSealed(sealed!)).toBe(true);
    // And the identity is unchanged — this is the entire point of migrating
    // rather than re-minting.
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(getPublicKey(load.key)).toBe(getPublicKey(SECRET));
  });

  it("opens the sealed key again on the next load", async () => {
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    await initGuardianKey();
    reload();
    await initGuardianKey();
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(Array.from(load.key)).toEqual(Array.from(SECRET));
  });

  it("is idempotent — a second init changes nothing", async () => {
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    await initGuardianKey();
    const sealed = localStorage.getItem(KEY_V2);
    await initGuardianKey();
    expect(localStorage.getItem(KEY_V2)).toBe(sealed);
  });

  /*
   * THE failure that must never lose a key. A WebView with IndexedDB off, a
   * blocked profile, storage denied: the vault cannot be built. The plaintext
   * key must stay exactly where it is and the app must keep working.
   */
  it("leaves the plaintext key alone when there is no vault to move it into", async () => {
    const nsec = nip19.nsecEncode(SECRET);
    localStorage.setItem(KEY_V1, nsec);
    vi.spyOn(crypto.subtle, "generateKey").mockRejectedValue(new Error("no crypto here"));

    await initGuardianKey();

    expect(localStorage.getItem(KEY_V1)).toBe(nsec);
    expect(localStorage.getItem(KEY_V2)).toBeFalsy();
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(Array.from(load.key)).toEqual(Array.from(SECRET));
  });

  /*
   * The subtler one: sealing APPEARS to work, but reading it back does not
   * produce the same bytes. Deleting v1 on the strength of an unverified write
   * is how a security improvement becomes the catastrophe it was guarding
   * against, so the read-back-and-compare has to be load-bearing.
   */
  it("keeps the plaintext when the sealed copy does not verify", async () => {
    const nsec = nip19.nsecEncode(SECRET);
    localStorage.setItem(KEY_V1, nsec);
    vi.spyOn(crypto.subtle, "decrypt").mockResolvedValue(new Uint8Array(32).fill(9).buffer);

    await initGuardianKey();

    expect(localStorage.getItem(KEY_V1), "unverified seal must not retire v1").toBe(nsec);
  });

  /*
   * THE bug this suite originally missed (found 2026-08-07 while reasoning
   * about deploy risk, before anything shipped).
   *
   * The first version asserted only that v1 SURVIVED a failed seal — which it
   * did. It never checked the next boot. A failed verification left the bad v2
   * blob in storage, `initGuardianKey` read v2 first and never looked at v1,
   * so the following load reported the guardian key CORRUPT and sent the
   * parent to restore-from-backup while their working key sat untouched in the
   * same storage. The safety net was the thing that dropped you.
   *
   * Two fixes, and both are asserted here: a half-written vault leaves no
   * trace, and an unopenable vault falls back to the plaintext rather than
   * declaring the key lost.
   */
  it("recovers on the NEXT load after a seal that did not verify", async () => {
    const nsec = nip19.nsecEncode(SECRET);
    localStorage.setItem(KEY_V1, nsec);
    const decrypt = vi
      .spyOn(crypto.subtle, "decrypt")
      .mockResolvedValue(new Uint8Array(32).fill(9).buffer);

    await initGuardianKey();
    expect(localStorage.getItem(KEY_V1)).toBe(nsec);
    expect(localStorage.getItem(KEY_V2), "no half-written blob left behind").toBeNull();

    // Next load, still broken: must still find the key, not cry corrupt.
    reload();
    await initGuardianKey();
    let load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(Array.from(load.key)).toEqual(Array.from(SECRET));

    // …and when whatever was wrong clears up, it migrates by itself.
    decrypt.mockRestore();
    reload();
    await initGuardianKey();
    await vi.waitFor(() => expect(localStorage.getItem(KEY_V1)).toBeNull());
    load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(Array.from(load.key)).toEqual(Array.from(SECRET));
  });

  it("falls back to the plaintext when a sealed blob will not open", async () => {
    // Both present — a migration interrupted between writing v2 and retiring
    // v1 by a closed tab, a crash, an OS kill.
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    localStorage.setItem(KEY_V2, "v2:AAAAAAAAAAAAAAAA:AAAAAAAAAAAAAAAAAAAAAAAA");

    await initGuardianKey();

    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(getPublicKey(load.key)).toBe(getPublicKey(SECRET));
  });

  it("retires a leftover plaintext once the vault opens cleanly", async () => {
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    await initGuardianKey();
    // Put the plaintext back, as an interrupted migration would have left it.
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    reload();

    await initGuardianKey();

    expect(localStorage.getItem(KEY_V1), "the job gets finished").toBeNull();
    expect(readGuardianKey().kind).toBe("ok");
  });

  it("never reports a sealed-but-unopenable key as absent", async () => {
    localStorage.setItem(KEY_V2, "v2:AAAAAAAAAAAAAAAA:AAAAAAAAAAAAAAAAAAAAAAAA");
    await initGuardianKey();
    // `corrupt` routes the parent to restore-from-backup. `absent` would mint
    // a brand-new identity on top and orphan every paired device.
    expect(readGuardianKey()).toEqual({ kind: "corrupt" });
  });

  it("reports a truly empty profile as absent", async () => {
    await initGuardianKey();
    expect(readGuardianKey()).toEqual({ kind: "absent" });
  });
});

describe("minting and restoring, with the vault present", () => {
  it("seals a freshly minted key", async () => {
    await initGuardianKey();
    const sk = loadOrCreateGuardianKey();
    // Minting persists plaintext first (a key that exists only in a pending
    // promise dies with a closed tab, and it is the one key with no backup),
    // then seals. Let that settle.
    await vi.waitFor(() => expect(localStorage.getItem(KEY_V1)).toBeNull());
    expect(isSealed(localStorage.getItem(KEY_V2)!)).toBe(true);

    reload();
    await initGuardianKey();
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(Array.from(load.key)).toEqual(Array.from(sk));
  });

  it("seals a restored backup", async () => {
    await initGuardianKey();
    importGuardianKey(SECRET);
    await vi.waitFor(() => expect(localStorage.getItem(KEY_V1)).toBeNull());

    reload();
    await initGuardianKey();
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") expect(getPublicKey(load.key)).toBe(getPublicKey(SECRET));
  });

  it("clear removes BOTH generations — a v1 left behind would resurrect it", async () => {
    localStorage.setItem(KEY_V1, nip19.nsecEncode(SECRET));
    await initGuardianKey();
    clearGuardianKey();
    expect(localStorage.getItem(KEY_V1)).toBeNull();
    expect(localStorage.getItem(KEY_V2)).toBeNull();
    expect(readGuardianKey()).toEqual({ kind: "absent" });
  });
});
