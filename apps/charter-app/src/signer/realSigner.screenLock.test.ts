import { beforeEach, describe, expect, it } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import { CHARTER_DEVICE_SCREEN_LOCK_RESET, type GuardianOps } from "../wire/giftwrap";
import { QueuedForHomeNetworkError } from "./routePublish";
import { RealSigner, type ChildTarget } from "./realSigner";

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

const DEV = generateSecretKey();
const DPK = getPublicKey(DEV);
const NOW = 1_700_000_000;
const RELAYS = ["wss://relay.one"];
const TARGET: ChildTarget = {
  subject: getPublicKey(generateSecretKey()),
  name: "Robin",
  devices: [{ id: "phone", pubkey: DPK }],
  relays: RELAYS,
};
const DEVICE = { machine: DPK, relays: RELAYS, label: "Robin’s phone" };

function setup(publishImpl?: () => Promise<void>) {
  const published: { relays: string[]; event: NostrEvent; meta?: unknown }[] = [];
  const asked: string[] = [];
  const signer = new RealSigner({
    connectGuardian: async () => guardian,
    publish: async (relays, event, meta) => {
      published.push({ relays, event, meta });
      await publishImpl?.();
    },
    resolveChild: () => TARGET,
    now: () => NOW,
    confirmGate: (ctx) => {
      asked.push(ctx.detail);
      return true;
    },
  });
  return { signer, published, asked };
}

/** Open a wrap the way the ward does: wrap → seal → guardian-signed inner. */
function open(wrap: NostrEvent) {
  const seal = JSON.parse(nip44.decrypt(wrap.content, nip44.getConversationKey(DEV, wrap.pubkey)));
  const inner = JSON.parse(nip44.decrypt(seal.content, nip44.getConversationKey(DEV, seal.pubkey)));
  return { seal, inner, payload: JSON.parse(inner.content) as Record<string, unknown> };
}

describe("RealSigner.signScreenLockReset", () => {
  it("signs the spec's inner content: v, 32-hex id, machine, wall-clock issuedAt, newPin", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    const out = await signer.signScreenLockReset(DEVICE, "123456");

    expect(published).toHaveLength(1);
    expect(published[0].relays).toEqual(RELAYS);
    const { inner, payload } = open(published[0].event);
    expect(inner.kind).toBe(CHARTER_DEVICE_SCREEN_LOCK_RESET);
    expect(inner.kind).toBe(31119);
    expect(inner.pubkey).toBe(GUARDIAN);
    expect(Object.keys(payload).sort()).toEqual(["id", "issuedAt", "machine", "newPin", "v"]);
    expect(payload.v).toBe(1);
    expect(payload.id).toMatch(/^[0-9a-f]{32}$/);
    expect(payload.machine).toBe(DPK);
    expect(payload.issuedAt).toBe(NOW);
    expect(typeof payload.issuedAt).toBe("number");
    expect(payload.newPin).toBe("123456");
    expect(out).toEqual({ ok: true, id: payload.id, issuedAt: NOW });
  });

  it("removes the lock with an empty newPin", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signScreenLockReset(DEVICE, "");
    expect(open(published[0].event).payload.newPin).toBe("");
  });

  it("uses a fresh id each time", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signScreenLockReset(DEVICE, "1234");
    await signer.signScreenLockReset(DEVICE, "1234");
    expect(open(published[0].event).payload.id).not.toBe(open(published[1].event).payload.id);
  });

  it("stamps the true clock, never the clause clock that can run ahead of it", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    // Three clauses inside one second push the clause clock to NOW + 2.
    for (let i = 0; i < 3; i++) await signer.signDebuggingClause("c", DPK, { allow: false });
    await signer.signScreenLockReset(DEVICE, "1234");
    expect(open(published[3].event).payload.issuedAt).toBe(NOW);
  });

  it("is wrapped to the device alone", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signScreenLockReset(DEVICE, "1234");
    expect(published[0].event.kind).toBe(1059);
    expect(published[0].event.tags).toContainEqual(["p", DPK]);
  });

  it("goes out like a release: one plain publish, no delivery tracking", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await signer.signScreenLockReset(DEVICE, "1234");
    expect(published).toHaveLength(1);
    expect(published[0].meta).toBeUndefined();
  });

  it("keeps the PIN out of the confirm sheet and out of the result", async () => {
    const { signer, asked } = setup();
    await signer.connect("local");
    const out = await signer.signScreenLockReset(DEVICE, "987654");
    expect(asked).toEqual(["Reset the screen lock on Robin’s phone to a new PIN"]);
    expect(JSON.stringify(asked)).not.toContain("987654");
    expect(JSON.stringify(out)).not.toContain("987654");
  });

  it("says 'remove' in the confirm sheet when removing", async () => {
    const { signer, asked } = setup();
    await signer.connect("local");
    await signer.signScreenLockReset(DEVICE, "");
    expect(asked).toEqual(["Remove the screen lock on Robin’s phone"]);
  });

  it.each([["123"], ["12345678901234567"], ["12a456"], [" 1234"], ["١٢٣٤"]])(
    "refuses a bad PIN %j without echoing it",
    async (bad) => {
      const { signer, published } = setup();
      await signer.connect("local");
      const err = await signer.signScreenLockReset(DEVICE, bad).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(Error);
      expect((err as Error).message).not.toContain(bad.trim() || "never");
      expect(published).toHaveLength(0);
    },
  );

  it("refuses a device with no real key", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await expect(signer.signScreenLockReset({ ...DEVICE, machine: "mock-key" }, "1234")).rejects.toThrow(
      /can't be reached/,
    );
    expect(published).toHaveLength(0);
  });

  it("a relay failure propagates without the PIN in it", async () => {
    const { signer } = setup(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    await signer.connect("local");
    const err = await signer.signScreenLockReset(DEVICE, "246810").catch((e: unknown) => e);
    expect((err as Error).message).not.toContain("246810");
  });

  it("counts a wrap the carrier is holding as sent", async () => {
    const { signer } = setup(async () => {
      throw new QueuedForHomeNetworkError(["Robin’s phone"]);
    });
    await signer.connect("local");
    await expect(signer.signScreenLockReset(DEVICE, "1234")).resolves.toMatchObject({ ok: true });
  });

  it("needs a connected signer", async () => {
    const { signer } = setup();
    await expect(signer.signScreenLockReset(DEVICE, "1234")).rejects.toThrow(/not connected/);
  });
});
