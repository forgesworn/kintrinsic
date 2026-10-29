// RealSigner under guardian roots and working keys: each ward's events are
// signed and sealed by the key `pickOps` chooses for it (W where the ward
// holds it, else the root), and a scan-to-pair offer goes out sealed by W
// with R's approval when the install has a Signet root.

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

  it("scan-to-pair: sealed by W with R's approval when the hook offers it", async () => {
    const published: NostrEvent[] = [];
    const approval = { kind: 31120, id: "x" };
    const s = signer(published, {
      pairOffer: (token, relays, ts) => ({ content: JSON.stringify({ token, relays, ts, approval }), ops: opsOf(wSk) }),
    });
    await s.connect("signet");
    await s.sendPairOffer(A, "ab".repeat(16), ["wss://one.example"]);
    const out = unwrapToWorkingKey(published[0], aSk)!;
    expect(out.sealAuthor).toBe(W);
    expect(out.inner.pubkey).toBe(W);
    expect(out.inner.kind).toBe(31117);
    expect(out.inner.tags).toEqual([["t", "charter-device"]]);
    expect(JSON.parse(out.inner.content)).toEqual({ token: "ab".repeat(16), relays: ["wss://one.example"], ts: 1_800_000_000, approval });
  });

  it("scan-to-pair falls back to the root's offer when the hook declines", async () => {
    const published: NostrEvent[] = [];
    const s = signer(published, { pairOffer: () => null });
    await s.connect("local");
    await s.sendPairOffer(A, "ab".repeat(16), ["wss://one.example"]);
    const out = unwrapToWorkingKey(published[0], aSk)!;
    expect(out.sealAuthor).toBe(R);
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
