import { describe, expect, it } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
} from "nostr-tools";
import { policyToClauses } from "./clause";
import { buildTimeExtendGrant } from "./grant";
import type { Policy } from "../domain/types";
import {
  CHARTER_DEVICE_RELEASE,
  giftWrapClause,
  giftWrapGrant,
  giftWrapRelease,
  unwrapClause,
  unwrapGrant,
  type GuardianOps,
} from "./giftwrap";

// A local guardian key stands in for Signet's guardian route: signEvent ==
// NIP-46 sign_event, nip44Encrypt == NIP-46 nip44_encrypt. The real signer
// delegates these two calls to the bunker; the gift-wrap logic is identical.
function localGuardian(sk: Uint8Array): GuardianOps {
  const pubkey = getPublicKey(sk);
  return {
    pubkey,
    async signEvent(t: EventTemplate) {
      return finalizeEvent(t, sk);
    },
    async nip44Encrypt(recipientPubkey: string, plaintext: string) {
      return nip44.encrypt(plaintext, nip44.getConversationKey(sk, recipientPubkey));
    },
  };
}

const devicePolicy: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: { tz: "Europe/London", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
  budget: { tz: "Europe/London", dailyMinutes: 90 },
};
const AT = 1_700_000_000;

describe("giftWrapClause / unwrapClause", () => {
  it("round-trips a CLAUSE: 1059 wrap → seal → signed inner 31113, author == guardian", async () => {
    const gsk = generateSecretKey();
    const guardian = localGuardian(gsk);
    const dsk = generateSecretKey();
    const dpk = getPublicKey(dsk);

    const [clause] = policyToClauses(dpk, devicePolicy, AT); // the schedule clause
    const wrap = await giftWrapClause(clause, dpk, guardian, AT);

    // The outer wrap is a 1059 addressed to the device by an EPHEMERAL key.
    expect(wrap.kind).toBe(1059);
    expect(wrap.pubkey).not.toBe(guardian.pubkey);
    expect(wrap.tags).toContainEqual(["p", dpk]);

    // The device unwraps with its key, verifies the inner sig + pinned author.
    const got = unwrapClause(wrap, dsk, guardian.pubkey);
    expect(got.valid).toBe(true);
    expect(got.inner.kind).toBe(31113);
    expect(got.inner.pubkey).toBe(guardian.pubkey);
    expect(got.payload.kind).toBe("schedule");
    expect(got.payload.subject).toBe(dpk);
    // The inner event is genuinely the guardian's signed event.
    expect(got.payload.body).toMatchObject({ v: 1, issuedAt: AT });
  });

  it("a clause forged by a NON-guardian key is detected (author mismatch)", async () => {
    const attacker = localGuardian(generateSecretKey());
    const dsk = generateSecretKey();
    const dpk = getPublicKey(dsk);
    const pinnedGuardian = getPublicKey(generateSecretKey()); // a DIFFERENT pinned key

    const [clause] = policyToClauses(dpk, devicePolicy, AT);
    const wrap = await giftWrapClause(clause, dpk, attacker, AT);

    const got = unwrapClause(wrap, dsk, pinnedGuardian);
    expect(got.valid).toBe(false); // inner author != pinned guardian
  });

  it("cannot be decrypted by the wrong recipient", async () => {
    const guardian = localGuardian(generateSecretKey());
    const dpk = getPublicKey(generateSecretKey());
    const wrongDevice = generateSecretKey();

    const [clause] = policyToClauses(dpk, devicePolicy, AT);
    const wrap = await giftWrapClause(clause, dpk, guardian, AT);

    expect(() => unwrapClause(wrap, wrongDevice, guardian.pubkey)).toThrow();
  });
});

const grantPayload = buildTimeExtendGrant({
  reqId: "ab".repeat(32),
  nonce: "cd".repeat(32),
  decision: "allow",
  minutesGranted: 25,
  limitHit: "schedule",
  ts: AT,
  tz: "UTC",
});

describe("giftWrapGrant / unwrapGrant", () => {
  it("round-trips a GRANT: 1059 wrap → seal → signed inner 31112, author == guardian", async () => {
    const guardian = localGuardian(generateSecretKey());
    const msk = generateSecretKey();
    const mpk = getPublicKey(msk);

    const wrap = await giftWrapGrant(grantPayload, mpk, guardian, AT);

    // The outer wrap is a 1059 addressed to the machine by an EPHEMERAL key.
    expect(wrap.kind).toBe(1059);
    expect(wrap.pubkey).not.toBe(guardian.pubkey);
    expect(wrap.tags).toContainEqual(["p", mpk]);

    // The device unwraps with its key, verifies the inner sig + pinned author,
    // and reads back the exact payload (reqId/nonce echo intact).
    const got = unwrapGrant(wrap, msk, guardian.pubkey);
    expect(got.valid).toBe(true);
    expect(got.inner.kind).toBe(31112);
    expect(got.inner.pubkey).toBe(guardian.pubkey);
    expect(got.payload).toEqual(grantPayload);
  });

  it("a grant forged by a NON-guardian key is detected (author mismatch)", async () => {
    const attacker = localGuardian(generateSecretKey());
    const msk = generateSecretKey();
    const pinnedGuardian = getPublicKey(generateSecretKey()); // a DIFFERENT pinned key

    const wrap = await giftWrapGrant(grantPayload, getPublicKey(msk), attacker, AT);

    const got = unwrapGrant(wrap, msk, pinnedGuardian);
    expect(got.valid).toBe(false); // inner author != pinned guardian
  });

  it("cannot be decrypted by the wrong recipient", async () => {
    const guardian = localGuardian(generateSecretKey());
    const mpk = getPublicKey(generateSecretKey());
    const wrongDevice = generateSecretKey();

    const wrap = await giftWrapGrant(grantPayload, mpk, guardian, AT);

    expect(() => unwrapGrant(wrap, wrongDevice, guardian.pubkey)).toThrow();
  });
});

describe("giftWrapRelease", () => {
  it("wraps a signed RELEASE (31116) to the machine, authored by the guardian", async () => {
    const gsk = generateSecretKey();
    const guardian = localGuardian(gsk);
    const dsk = generateSecretKey();
    const dpk = getPublicKey(dsk);
    const wrap = await giftWrapRelease(dpk, guardian, AT);
    expect(wrap.kind).toBe(1059);
    expect(wrap.tags).toContainEqual(["p", dpk]);
    // The device opens it exactly like a clause (same envelope): wrap → seal →
    // guardian-signed inner 31116 whose payload targets THIS machine.
    const seal = JSON.parse(nip44.decrypt(wrap.content, nip44.getConversationKey(dsk, wrap.pubkey)));
    expect(seal.kind).toBe(13);
    const inner = JSON.parse(nip44.decrypt(seal.content, nip44.getConversationKey(dsk, seal.pubkey)));
    expect(inner.kind).toBe(CHARTER_DEVICE_RELEASE);
    expect(inner.pubkey).toBe(guardian.pubkey); // guardian authored it
    expect(JSON.parse(inner.content)).toEqual({ v: 1, machine: dpk, issuedAt: AT });
  });
});
