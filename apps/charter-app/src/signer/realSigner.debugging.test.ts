import { beforeEach, describe, expect, it } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import { unwrapClause, type GuardianOps } from "../wire/giftwrap";
import { RealSigner, type ChildTarget } from "./realSigner";

// The signer persists its clause clock; with a frozen `now` it would leak between tests.
beforeEach(() => localStorage.clear());

const GSK = generateSecretKey();
const GUARDIAN = getPublicKey(GSK);
const guardian: GuardianOps = {
  pubkey: GUARDIAN,
  async signEvent(t: EventTemplate) {
    return finalizeEvent(t, GSK);
  },
  async nip44Encrypt(recipient: string, plaintext: string) {
    return nip44.encrypt(plaintext, nip44.getConversationKey(GSK, recipient));
  },
};

const DEV1 = generateSecretKey();
const DEV2 = generateSecretKey();
const DPK1 = getPublicKey(DEV1);
const DPK2 = getPublicKey(DEV2);
const SUBJECT = getPublicKey(generateSecretKey());
const RELAYS = ["wss://relay.one", "wss://relay.two"];
const TARGET: ChildTarget = {
  subject: SUBJECT,
  name: "Robin",
  devices: [
    { id: "laptop", pubkey: DPK1 },
    { id: "phone", pubkey: DPK2 },
  ],
  relays: RELAYS,
};

function setup(target: ChildTarget = TARGET) {
  const published: { relays: string[]; event: NostrEvent }[] = [];
  const asked: string[] = [];
  const signer = new RealSigner({
    connectGuardian: async () => guardian,
    publish: async (relays, event) => {
      published.push({ relays, event });
    },
    resolveChild: () => target,
    deviceName: (m) => (m === DPK2 ? "Robin’s phone" : "Robin’s laptop"),
    now: () => 1_700_000_000,
    confirmGate: (ctx) => {
      asked.push(ctx.detail);
      return true;
    },
  });
  return { signer, published, asked };
}

function readFor(published: { event: NostrEvent }[], sk: Uint8Array) {
  return published.flatMap((p) => {
    try {
      return [unwrapClause(p.event, sk, GUARDIAN)];
    } catch {
      return [];
    }
  });
}

describe("RealSigner.signDebuggingClause", () => {
  it("wraps ONLY the named device, keeping the child subject", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signDebuggingClause("c", DPK2, { allow: true, until: 1_700_003_600 });

    expect(published).toHaveLength(1);
    expect(readFor(published, DEV1)).toHaveLength(0); // the other device cannot read it
    const [u] = readFor(published, DEV2);
    expect(u.valid).toBe(true);
    expect(u.payload.kind).toBe("debugging");
    expect(u.payload.subject).toBe(SUBJECT);
    expect(u.payload.body).toEqual({ v: 1, issuedAt: u.payload.issuedAt, allow: true, until: 1_700_003_600 });
  });

  it("returns the issuedAt it signed with, in unix seconds", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    const out = await signer.signDebuggingClause("c", DPK2, { allow: true });
    const [u] = readFor(published, DEV2);
    expect(out.issuedAt).toBe(u.payload.issuedAt);
    expect(out.issuedAt).toBe(1_700_000_000);
  });

  it("drops `until` when turning debugging off", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signDebuggingClause("c", DPK1, { allow: false, until: 1_700_003_600 });
    const [u] = readFor(published, DEV1);
    expect(u.payload.body).toEqual({ v: 1, issuedAt: u.payload.issuedAt, allow: false });
  });

  it("refuses a device that is not the child's", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await expect(
      signer.signDebuggingClause("c", getPublicKey(generateSecretKey()), { allow: true }),
    ).rejects.toThrow(/isn't paired/);
    expect(published).toHaveLength(0);
  });

  it("supersedes: a later clause carries a strictly higher issuedAt", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signDebuggingClause("c", DPK2, { allow: true });
    await signer.signDebuggingClause("c", DPK2, { allow: false });
    const [on, off] = readFor(published, DEV2);
    expect(off.payload.issuedAt).toBeGreaterThan(on.payload.issuedAt);
  });

  it("asks the guardian in plain words, naming the device", async () => {
    const { signer, asked } = setup();
    await signer.connect("local");
    await signer.signDebuggingClause("c", DPK2, { allow: true });
    await signer.signDebuggingClause("c", DPK2, { allow: false });
    await signer.signDebuggingClause("c", DPK2, { allow: true, until: 1_700_003_600 });
    expect(asked[0]).toBe("Turn on USB debugging on Robin’s phone until you turn it off");
    expect(asked[1]).toBe("Turn off USB debugging on Robin’s phone");
    expect(asked[2]).toMatch(/^Turn on USB debugging on Robin’s phone until /);
  });
});
