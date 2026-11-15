// Cross-language parity for the guardian's SCREEN_LOCK_RESET: the SAME vector
// file the Rust ward asserts (core/crates/charter-verify/tests/
// screen_lock_reset_vectors.rs, core/crates/charter-transport/tests/
// screen_lock_seal_vectors.rs, and a ward holding the vector's machine key in
// android/jni) must be what this guardian signs, byte for byte: the same
// sealed PIN from the same one-use key and nonce, the same kind, tags and
// content (keys, order, types).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import {
  CHARTER_DEVICE_SCREEN_LOCK_RESET,
  giftWrapScreenLockReset,
  sealScreenLockPin,
  type GuardianOps,
  type ScreenLockResetPayload,
} from "./giftwrap";
import { RealSigner } from "../signer/realSigner";

interface Vector {
  name: string;
  newPin: string;
  plaintext: string;
  nonce: string;
  payload: ScreenLockResetPayload;
  content: string;
  removesScreenLock: boolean;
}
interface SealFailure {
  name: string;
  newPin?: string;
  plaintext?: string;
  sealedTo?: string;
  nonce?: string;
  payload: ScreenLockResetPayload;
  content: string;
  error: string;
}
interface VectorFile {
  kind: number;
  tags: string[][];
  keys: {
    machineSecretKey: string;
    machine: string;
    ephemeralSecretKey: string;
    epk: string;
    otherMachineSecretKey: string;
    otherMachine: string;
  };
  issuedAt: number;
  vectors: Vector[];
  sealFailures: SealFailure[];
  invalid: { name: string; payload: Record<string, unknown> }[];
}

// vitest cwd is apps/charter-app; import.meta.url is not file:// under jsdom.
const file: VectorFile = JSON.parse(
  readFileSync(
    join(process.cwd(), "../../core/crates/charter-testkit/vectors/screen_lock/screen_lock_reset_vectors.json"),
    "utf8",
  ),
);

const WIRE_ORDER = ["v", "id", "machine", "issuedAt", "epk", "newPinSealed"];
const bytes = (hex: string) => Uint8Array.from(hex.match(/../g)!.map((h) => parseInt(h, 16)));
const MACHINE_SK = bytes(file.keys.machineSecretKey);
const fixed = (nonce: string) => ({
  ephemeralSecretKey: bytes(file.keys.ephemeralSecretKey),
  nonce: bytes(nonce),
});

/** A guardian that records every inner event it signs (the seal, kind 13, is skipped). */
function recordingGuardian() {
  const sk = generateSecretKey();
  const inners: NostrEvent[] = [];
  const guardian: GuardianOps = {
    pubkey: getPublicKey(sk),
    async signEvent(t: EventTemplate) {
      const ev = finalizeEvent(t, sk);
      if (t.kind !== 13) inners.push(ev);
      return ev;
    },
    // The seal's content is not under test; the inner event is.
    async nip44Encrypt(_recipient: string, plaintext: string) {
      return plaintext;
    },
  };
  return { guardian, inners };
}

function expectInnerMatches(inner: NostrEvent, v: Vector) {
  expect(inner.kind).toBe(file.kind);
  expect(inner.kind).toBe(31122);
  expect(inner.kind).toBe(CHARTER_DEVICE_SCREEN_LOCK_RESET);
  expect(inner.tags).toEqual(file.tags);
  expect(inner.created_at).toBe(v.payload.issuedAt);
  // Byte-identical: the same keys, in the same order, with the same types.
  expect(inner.content).toBe(v.content);
  const parsed = JSON.parse(inner.content) as Record<string, unknown>;
  expect(Object.keys(parsed)).toEqual(WIRE_ORDER);
  expect(parsed).toEqual(v.payload);
  expect(typeof parsed.v).toBe("number");
  expect(typeof parsed.issuedAt).toBe("number");
  for (const k of ["id", "machine", "epk", "newPinSealed"]) expect(typeof parsed[k]).toBe("string");
}

/** Open a vector's seal as the ward does, with the vector's machine key. */
function openSeal(p: ScreenLockResetPayload): string {
  return nip44.decrypt(p.newPinSealed, nip44.getConversationKey(MACHINE_SK, p.epk));
}

describe("the shared screen-lock reset vector file", () => {
  it("names kind 31122, real fixed keys, and valid, seal-failure and invalid cases", () => {
    expect(file.kind).toBe(31122);
    expect(getPublicKey(MACHINE_SK)).toBe(file.keys.machine);
    expect(getPublicKey(bytes(file.keys.ephemeralSecretKey))).toBe(file.keys.epk);
    expect(getPublicKey(bytes(file.keys.otherMachineSecretKey))).toBe(file.keys.otherMachine);
    expect(file.vectors.length).toBeGreaterThan(0);
    expect(file.sealFailures.some((f) => f.sealedTo === file.keys.otherMachine)).toBe(true);
    expect(file.invalid.length).toBeGreaterThan(0);
  });

  it.each(file.vectors.map((v) => [v.name, v] as const))(
    "sealScreenLockPin makes the vector's seal exactly from its fixed key and nonce: %s",
    (_name, v) => {
      const sealed = sealScreenLockPin(v.newPin, file.keys.machine, fixed(v.nonce));
      expect(sealed).toEqual({ epk: v.payload.epk, newPinSealed: v.payload.newPinSealed });
      expect(JSON.stringify(v.newPin)).toBe(v.plaintext);
      expect(openSeal(v.payload)).toBe(v.plaintext);
      expect(v.removesScreenLock).toBe(v.newPin === "");
    },
  );

  it.each(file.sealFailures.filter((f) => f.plaintext && f.sealedTo).map((f) => [f.name, f] as const))(
    "the seal-failure ciphertext is reproducible: %s",
    (_name, f) => {
      const key = nip44.getConversationKey(bytes(file.keys.ephemeralSecretKey), f.sealedTo!);
      expect(nip44.encrypt(f.plaintext!, key, bytes(f.nonce!))).toBe(f.payload.newPinSealed);
    },
  );

  it.each(file.vectors.map((v) => [v.name, v] as const))(
    "giftWrapScreenLockReset signs the vector's content exactly: %s",
    async (_name, v) => {
      const { guardian, inners } = recordingGuardian();
      await giftWrapScreenLockReset({ ...v.payload, v: 1 }, guardian);
      expect(inners).toHaveLength(1);
      expectInnerMatches(inners[0], v);
    },
  );
});

describe("RealSigner.signScreenLockReset against the shared vectors", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it.each(file.vectors.map((v) => [v.name, v] as const))(
    "signs the vector's content exactly for its inputs: %s",
    async (_name, v) => {
      const { guardian, inners } = recordingGuardian();
      const signer = new RealSigner({
        connectGuardian: async () => guardian,
        publish: async () => {},
        resolveChild: () => ({ subject: null, name: "Robin", devices: [], relays: [] }),
        now: () => v.payload.issuedAt,
        confirmGate: () => true,
      });
      await signer.connect("local");
      // The signer draws, in order: the 16-byte id, the one-use key, the
      // nonce. Serve those three from the vector; anything later (the gift
      // wrap's own randomness) is real.
      const draws = [
        bytes(v.payload.id),
        bytes(file.keys.ephemeralSecretKey),
        bytes(v.nonce),
      ];
      const real = crypto.getRandomValues.bind(crypto);
      vi.spyOn(crypto, "getRandomValues").mockImplementation(((a: Uint8Array) => {
        const next = draws.shift();
        if (!next) return real(a);
        expect(a.length).toBe(next.length);
        a.set(next);
        return a;
      }) as typeof crypto.getRandomValues);

      const out = await signer.signScreenLockReset(
        { machine: v.payload.machine, relays: ["wss://relay.one"], label: "Robin’s phone" },
        v.newPin,
      );
      expect(draws).toHaveLength(0);
      expect(out).toEqual({ ok: true, id: v.payload.id, issuedAt: v.payload.issuedAt });
      expect(inners).toHaveLength(1);
      expectInnerMatches(inners[0], v);
    },
  );

  it("refuses every PIN the ward would refuse, before sealing anything", async () => {
    const { guardian, inners } = recordingGuardian();
    const signer = new RealSigner({
      connectGuardian: async () => guardian,
      publish: async () => {},
      resolveChild: () => ({ subject: null, name: "Robin", devices: [], relays: [] }),
      now: () => file.issuedAt,
      confirmGate: () => true,
    });
    await signer.connect("local");
    const badPins = file.sealFailures
      .map((c) => c.newPin)
      .filter((p): p is string => typeof p === "string");
    expect(badPins.length).toBeGreaterThan(0);
    for (const newPin of badPins) {
      expect(() => sealScreenLockPin(newPin, file.keys.machine)).toThrow();
      await expect(
        signer.signScreenLockReset({ machine: file.keys.machine, relays: ["wss://relay.one"] }, newPin),
      ).rejects.toThrow();
    }
    expect(inners).toHaveLength(0);
  });
});
