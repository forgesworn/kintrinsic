// RealSigner under guardian roots and working keys: each ward's events are
// signed and sealed by the key `pickOps` chooses for it (W where the ward
// holds it, else the root); and a scan-to-pair offer always goes out from
// the root.

import { beforeEach, describe, expect, it } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip44, type NostrEvent } from "nostr-tools";
import type { GuardianOps } from "../wire/giftwrap";
import type { Policy } from "../domain/types";
import { RealSigner, type ChildTarget } from "./realSigner";
import { unwrapToWorkingKey } from "../keys/guardianKeys";
import { selectSigner } from "./selectSigner";

beforeEach(() => localStorage.clear());

function opsOf(sk: Uint8Array): GuardianOps {
  return {
    pubkey: getPublicKey(sk),
    signEvent: async (t) => finalizeEvent(t, sk),
    nip44Encrypt: async (to, pt) => nip44.encrypt(pt, nip44.getConversationKey(sk, to)),
  };
}

const rSk = generateSecretKey();
const wSk = generateSecretKey();
const R = getPublicKey(rSk);
const W = getPublicKey(wSk);
const aSk = generateSecretKey();
const bSk = generateSecretKey();
const A = getPublicKey(aSk);
const B = getPublicKey(bSk);

const target: ChildTarget = {
  subject: null,
  name: "Sam",
  devices: [
    { id: "d1", pubkey: A, platform: "android" },
    { id: "d2", pubkey: B, platform: "linux" },
  ],
  relays: ["wss://one.example", "wss://two.example"],
};
const policy: Policy = { id: "p1", scope: { kind: "device" }, budget: { tz: "Europe/London", dailyMinutes: 60 } };

function signer(published: NostrEvent[], extra: Partial<ConstructorParameters<typeof RealSigner>[0]> = {}) {
  return new RealSigner({
    connectGuardian: async () => opsOf(rSk),
    publish: async (_r, e) => {
      published.push(e);
    },
    resolveChild: () => target,
    now: () => 1_800_000_000,
    ...extra,
  });
}

const sk = (e: NostrEvent) => (e.tags.find((t) => t[0] === "p")![1] === A ? aSk : bSk);

describe("RealSigner with working keys", () => {
  it("signs and seals each ward's clause with the key pickOps chooses", async () => {
    const published: NostrEvent[] = [];
    const s = signer(published, { pickOps: (m) => (m === A ? opsOf(wSk) : undefined) });
    await s.connect("signet");
    await s.signClause("c1", policy);
    expect(published.length).toBeGreaterThan(0);
    for (const e of published) {
      const out = unwrapToWorkingKey(e, sk(e))!;
      const expected = e.tags.find((t) => t[0] === "p")![1] === A ? W : R;
      expect(out.sealAuthor).toBe(expected);
      expect(out.inner.pubkey).toBe(expected);
    }
    expect(s.rootOps()?.pubkey).toBe(R);
  });

  it("without pickOps, every ward gets the root's signature as before", async () => {
    const published: NostrEvent[] = [];
    const s = signer(published);
    await s.connect("signet");
    await s.signClause("c1", policy);
    for (const e of published) expect(unwrapToWorkingKey(e, sk(e))!.inner.pubkey).toBe(R);
  });

  it("scan-to-pair is offered by the root, never by W, whatever pickOps says", async () => {
    // A ward that predates guardian keys pins the seal author: a W-sealed
    // offer would pin W, and the app can't tell the ward's version first.
    const published: NostrEvent[] = [];
    const s = signer(published, { pickOps: () => opsOf(wSk) });
    await s.connect("signet");
    await s.sendPairOffer(A, "ab".repeat(16), ["wss://one.example"]);
    const out = unwrapToWorkingKey(published[0], aSk)!;
    expect(out.sealAuthor).toBe(R);
    expect(out.inner.pubkey).toBe(R);
    expect(out.inner.kind).toBe(31117);
    expect(JSON.parse(out.inner.content).approval).toBeUndefined();
  });
});

describe("selectSigner without a signer set up", () => {
  it("is a disconnected signer that refuses to sign outside development", async () => {
    const s = selectSigner(
      { connected: false, kind: "none", autoSign: true },
      { resolveChild: () => undefined, childPolicies: () => [], confirmGate: () => true, allowMock: false },
    );
    expect(s.status()).toEqual({ connected: false, kind: "none", autoSign: true });
    await expect(s.connect("heartwood")).rejects.toThrow(/parent approval/);
    await expect(s.signClause("c1", policy)).rejects.toThrow(/parent approval/);
    await expect(s.signDecision("r1", "approved")).rejects.toThrow(/parent approval/);
  });
});
