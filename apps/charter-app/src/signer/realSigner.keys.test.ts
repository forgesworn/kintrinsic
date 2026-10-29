// RealSigner under guardian roots and working keys: each ward's events are
// signed and sealed by the key `pickOps` chooses for it (W where the ward
// holds it, else the root); a scan-to-pair offer always goes out from the
// root; and rule 4's re-sign of the standing one-off clauses.

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

describe("rule 4: re-signing standing clauses", () => {
  it("re-signs the last gift, stand-down and debugging per device with a fresh issuedAt and the same id", async () => {
    const published: NostrEvent[] = [];
    let now = 1_800_000_000;
    const s = signer(published, { now: () => now });
    await s.connect("signet");
    await s.signGiftClause("c1", 15);
    await s.signStandDownClause("c1");
    await s.signDebuggingClause("c1", B, { allow: true, until: now + 3600 });
    const first = published.map((e) => unwrapToWorkingKey(e, sk(e))!);
    const giftId = (JSON.parse(first[0].inner.content) as { body: { id: string } }).body.id;
    published.length = 0;
    now += 100;
    await s.resignStanding("c1");
    const again = published.map((e) => ({ to: e.tags.find((t) => t[0] === "p")![1], c: JSON.parse(unwrapToWorkingKey(e, sk(e))!.inner.content) }));
    // gift + standdown to both devices, debugging to B only
    expect(again.map((x) => `${x.c.kind}:${x.to === A ? "A" : "B"}`).sort()).toEqual(
      ["debugging:B", "gift:A", "gift:B", "standdown:A", "standdown:B"],
    );
    for (const x of again) {
      expect(x.c.issuedAt).toBeGreaterThan(1_800_000_000 + 99);
      if ("issuedAt" in x.c.body) expect(x.c.body.issuedAt).toBe(x.c.issuedAt);
    }
    expect(again.filter((x) => x.c.kind === "gift").every((x) => x.c.body.id === giftId)).toBe(true);
  });

  it("does not record a clause whose send failed outright, and re-signs nothing when nothing is recorded", async () => {
    const published: NostrEvent[] = [];
    const s = signer(published, {
      publish: async () => {
        throw new Error("no relay");
      },
    });
    await s.connect("signet");
    await expect(s.signGiftClause("c1", 15)).rejects.toThrow();
    const ok = signer(published);
    await ok.connect("signet");
    await expect(ok.resignStanding("c1")).resolves.toEqual({ ok: true });
    expect(published).toHaveLength(0);
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
