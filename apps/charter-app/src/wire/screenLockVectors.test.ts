// Cross-language parity for the guardian's SCREEN_LOCK_RESET: the SAME vector
// file the Rust ward asserts (core/crates/charter-verify/tests/
// screen_lock_reset_vectors.rs) must be what this guardian signs, byte for
// byte: same kind, same tags, same content (keys, order, types).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, type EventTemplate, type NostrEvent } from "nostr-tools";
import { CHARTER_DEVICE_SCREEN_LOCK_RESET, giftWrapScreenLockReset, type GuardianOps } from "./giftwrap";
import { RealSigner } from "../signer/realSigner";

interface Payload {
  v: number;
  id: string;
  machine: string;
  issuedAt: number;
  newPin: string;
}
interface Vector {
  name: string;
  payload: Payload;
  content: string;
  removesScreenLock: boolean;
}
interface VectorFile {
  kind: number;
  tags: string[][];
  vectors: Vector[];
  invalid: { name: string; payload: Record<string, unknown> }[];
}

// vitest cwd is apps/charter-app; import.meta.url is not file:// under jsdom.
const file: VectorFile = JSON.parse(
  readFileSync(
    join(process.cwd(), "../../core/crates/charter-testkit/vectors/screen_lock/screen_lock_reset_vectors.json"),
    "utf8",
  ),
);

const WIRE_ORDER = ["v", "id", "machine", "issuedAt", "newPin"];

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
  for (const k of ["id", "machine", "newPin"]) expect(typeof parsed[k]).toBe("string");
}

describe("the shared screen-lock reset vector file", () => {
  it("names kind 31122 and has valid and invalid cases", () => {
    expect(file.kind).toBe(31122);
    expect(file.vectors.length).toBeGreaterThan(0);
    expect(file.invalid.length).toBeGreaterThan(0);
  });

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
      // The signer draws a random id: make that draw the vector's id.
      const idBytes = Uint8Array.from(v.payload.id.match(/../g)!.map((h) => parseInt(h, 16)));
      vi.spyOn(crypto, "getRandomValues").mockImplementation(((a: Uint8Array) => {
        a.set(idBytes);
        return a;
      }) as typeof crypto.getRandomValues);

      const out = await signer.signScreenLockReset(
        { machine: v.payload.machine, relays: ["wss://relay.one"], label: "Robin’s phone" },
        v.payload.newPin,
      );
      expect(out).toEqual({ ok: true, id: v.payload.id, issuedAt: v.payload.issuedAt });
      expect(inners).toHaveLength(1);
      expectInnerMatches(inners[0], v);
    },
  );

  it("refuses every vector PIN the ward would refuse", async () => {
    const { guardian, inners } = recordingGuardian();
    const signer = new RealSigner({
      connectGuardian: async () => guardian,
      publish: async () => {},
      resolveChild: () => ({ subject: null, name: "Robin", devices: [], relays: [] }),
      now: () => 1_782_734_400,
      confirmGate: () => true,
    });
    await signer.connect("local");
    const machine = "ab".repeat(32);
    const badPins = file.invalid
      .map((c) => c.payload.newPin)
      .filter((p): p is string => typeof p === "string");
    expect(badPins.length).toBeGreaterThan(0);
    for (const newPin of badPins) {
      // The valid PIN "123456" appears in the vector's id/version/machine cases:
      // those fail on other fields the signer builds itself, so skip them here.
      if (/^[0-9]{4,16}$/.test(newPin)) continue;
      await expect(
        signer.signScreenLockReset({ machine, relays: ["wss://relay.one"] }, newPin),
      ).rejects.toThrow();
    }
    expect(inners).toHaveLength(0);
  });
});
