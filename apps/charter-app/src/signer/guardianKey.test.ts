import { afterEach, describe, expect, it } from "vitest";
import { getPublicKey, nip19 } from "nostr-tools";
import {
  clearGuardianKey,
  CorruptGuardianKeyError,
  guardianPubkeyHex,
  loadOrCreateGuardianKey,
  readGuardianKey,
} from "./guardianKey";

// The private localStorage key `guardianKey.ts` persists under. Referenced here
// (not exported) so the tests can plant a corrupt value the way a real broken
// storage blob would present.
const STORAGE_KEY = "charter.guardian.key.v1";

afterEach(() => clearGuardianKey());

describe("guardianKey", () => {
  it("persists one key — same pubkey across calls", () => {
    const a = getPublicKey(loadOrCreateGuardianKey());
    expect(guardianPubkeyHex()).toBe(a);
  });

  it("regenerates a different key after clear (the revocation property)", () => {
    const first = guardianPubkeyHex();
    clearGuardianKey();
    const second = guardianPubkeyHex();
    expect(second).not.toBe(first);
    expect(second).toHaveLength(64);
  });
});

describe("guardianKey — absent vs corrupt (the key-loss trap)", () => {
  it("reports absent on a fresh install (nothing stored)", () => {
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
    expect(readGuardianKey()).toEqual({ kind: "absent" });
  });

  it("fresh install still mints + persists a key", () => {
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull();
    const sk = loadOrCreateGuardianKey();
    expect(sk).toHaveLength(32);
    const stored = localStorage.getItem(STORAGE_KEY);
    expect(stored).toBeTruthy();
    const load = readGuardianKey();
    expect(load.kind).toBe("ok");
    if (load.kind === "ok") {
      expect(Array.from(load.key)).toEqual(Array.from(sk));
    }
  });

  it("a valid stored key loads without minting a new one", () => {
    const first = loadOrCreateGuardianKey();
    const stored = localStorage.getItem(STORAGE_KEY);
    const again = loadOrCreateGuardianKey();
    expect(Array.from(again)).toEqual(Array.from(first));
    // Loading an existing key must not rewrite storage.
    expect(localStorage.getItem(STORAGE_KEY)).toBe(stored);
  });

  it("reports a corrupt stored value as corrupt — and NEVER overwrites it", () => {
    const garbage = "not-an-nsec-🙅";
    localStorage.setItem(STORAGE_KEY, garbage);
    expect(readGuardianKey()).toEqual({ kind: "corrupt" });
    // The load path must refuse to mint over a present-but-broken key…
    expect(() => loadOrCreateGuardianKey()).toThrow(CorruptGuardianKeyError);
    expect(() => guardianPubkeyHex()).toThrow(CorruptGuardianKeyError);
    // …and the corrupt bytes survive untouched — no silent new identity.
    expect(localStorage.getItem(STORAGE_KEY)).toBe(garbage);
  });

  it("treats a well-formed but non-nsec value as corrupt, not absent", () => {
    // An npub decodes cleanly but is the WRONG type: present, undecodable AS a
    // secret key. It must NOT be mistaken for a fresh install and minted over.
    const npub = nip19.npubEncode("f".repeat(64));
    localStorage.setItem(STORAGE_KEY, npub);
    expect(readGuardianKey()).toEqual({ kind: "corrupt" });
    expect(() => loadOrCreateGuardianKey()).toThrow(CorruptGuardianKeyError);
    expect(localStorage.getItem(STORAGE_KEY)).toBe(npub);
  });
});
