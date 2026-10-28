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
import { RELEASE_PUBKEY_HEX } from "../release/releaseTrust";
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

const LAPTOP = generateSecretKey();
const PHONE = generateSecretKey();
const LPK = getPublicKey(LAPTOP);
const PPK = getPublicKey(PHONE);
const SUBJECT = getPublicKey(generateSecretKey());
const TARGET: ChildTarget = {
  subject: SUBJECT,
  name: "Robin",
  devices: [
    { id: "laptop", pubkey: LPK },
    { id: "phone", pubkey: PPK },
  ],
  relays: ["wss://relay.one"],
};
const DEB = { versionCode: 712, versionName: "0.7.12", sha256: "3e".repeat(32) };
const URL = "https://github.com/forgesworn/kintrinsic/releases/download/linux-v0.7.12/kintrinsic_0.7.12_amd64.deb";

function setup() {
  const published: { relays: string[]; event: NostrEvent }[] = [];
  const asked: string[] = [];
  const signer = new RealSigner({
    connectGuardian: async () => guardian,
    publish: async (relays, event) => {
      published.push({ relays, event });
    },
    resolveChild: () => TARGET,
    deviceName: (m) => (m === LPK ? "Robin’s laptop" : "Robin’s phone"),
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

describe("RealSigner.signDebUpdateClause", () => {
  it("wraps the Linux update clause to the laptop alone, per the contract", async () => {
    const { signer, published, asked } = setup();
    await signer.connect("local");
    await signer.signDebUpdateClause("c", LPK, DEB, URL);

    expect(published).toHaveLength(1);
    // The phone keeps its own `update` slot.
    expect(readFor(published, PHONE)).toHaveLength(0);
    const [u] = readFor(published, LAPTOP);
    expect(u.valid).toBe(true);
    expect(u.payload.kind).toBe("update");
    expect(u.payload.subject).toBe(SUBJECT);
    expect(u.payload.body).toEqual({
      v: 1,
      packageName: "kintrinsic",
      versionCode: 712,
      versionName: "0.7.12",
      url: URL,
      apkSha256: DEB.sha256,
      signerCertSha256: RELEASE_PUBKEY_HEX,
    });
    expect(asked).toEqual(["Install Kintrinsic 0.7.12 on Robin’s laptop"]);
  });

  it("refuses a device that is not the child's", async () => {
    const { signer, published } = setup();
    await signer.connect("local");
    await expect(
      signer.signDebUpdateClause("c", getPublicKey(generateSecretKey()), DEB, URL),
    ).rejects.toThrow(/isn't paired/);
    expect(published).toHaveLength(0);
  });
});
