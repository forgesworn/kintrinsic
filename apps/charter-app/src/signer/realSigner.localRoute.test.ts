import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import type { GuardianOps } from "../wire/giftwrap";
import type { DecisionContext } from "./Signer";
import { RealSigner, type ChildTarget } from "./realSigner";
import { QueuedForHomeNetworkError, withLocalRoute, type PublishMeta } from "./routePublish";

beforeEach(() => localStorage.clear());
afterEach(() => {
  delete window.CharterCarrier;
  vi.restoreAllMocks();
});

const GSK = generateSecretKey();
const GUARDIAN = getPublicKey(GSK);
function localGuardian(): GuardianOps {
  return {
    pubkey: GUARDIAN,
    async signEvent(t: EventTemplate) {
      return finalizeEvent(t, GSK);
    },
    async nip44Encrypt(recipient: string, plaintext: string) {
      return nip44.encrypt(plaintext, nip44.getConversationKey(GSK, recipient));
    },
  };
}

const DPK1 = getPublicKey(generateSecretKey());
const DPK2 = getPublicKey(generateSecretKey());
const SUBJECT = getPublicKey(generateSecretKey());
const RELAYS = ["wss://relay.one", "wss://relay.two"];

function installBridge(sendFor: (recipient: string) => string) {
  // A faithful little carrier: a wrap it sent is soon received by the ward; one
  // it queued stays queued.
  const stateOf = new Map<string, string>();
  const send = vi.fn((recipient: string, wrapJson: string) => {
    const state = sendFor(recipient);
    stateOf.set((JSON.parse(wrapJson) as { id: string }).id, state === "sent" ? "received" : state);
    return JSON.stringify({ state });
  });
  window.CharterCarrier = {
    provision: vi.fn(),
    isCarrier: () => true,
    localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
    localSend: send,
    localDelivery: (idsJson: string) =>
      JSON.stringify(
        Object.fromEntries(
          (JSON.parse(idsJson) as string[])
            .filter((id) => stateOf.has(id))
            .map((id) => [id, { state: stateOf.get(id), at: 1, machine: DPK1 }]),
        ),
      ),
    localInbox: () => "[]",
    localConsume: vi.fn(),
  };
  return send;
}

function makeSigner(target: ChildTarget, relayPublish: (relays: string[], e: NostrEvent) => Promise<void>) {
  const names: Record<string, string> = { [DPK1]: "Robin's laptop", [DPK2]: "Mia's phone" };
  const routed: { wrapId: string; machine: string; meta: PublishMeta }[] = [];
  const signer = new RealSigner({
    connectGuardian: async () => localGuardian(),
    publish: withLocalRoute(relayPublish, {
      deviceName: (m) => names[m],
      onRouted: (w) => routed.push({ wrapId: w.wrapId, machine: w.machine, meta: w.meta }),
    }),
    resolveChild: () => target,
    now: () => 1_700_000_000,
    confirmGate: () => true,
  });
  return { signer, routed };
}

describe("requireDeliverable with a home-network route", () => {
  it("allows zero relays when the local route is available", async () => {
    installBridge(() => "sent");
    const relay = vi.fn(async () => {});
    const { signer } = makeSigner({ subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: [] }, relay);
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).resolves.toEqual({ ok: true });
    expect(relay).not.toHaveBeenCalled(); // nothing to publish to, and none needed
  });

  it("still refuses zero relays without a local route, with the same words as before", async () => {
    const { signer } = makeSigner({ subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: [] }, async () => {});
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow(/no relay is set up/i);
  });

  it("still refuses zero relays on a carrier whose route is not running", async () => {
    installBridge(() => "sent");
    window.CharterCarrier!.localInfo = () => JSON.stringify({ v: 1, available: false, listening: false, sessions: [] });
    const { signer } = makeSigner({ subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: [] }, async () => {});
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow(/no relay is set up/i);
  });

  it("still refuses a ward with no paired device, local route or not", async () => {
    installBridge(() => "sent");
    const { signer } = makeSigner({ subject: SUBJECT, devices: [], relays: [] }, async () => {});
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow(/no paired device/i);
  });
});

describe("a batch with a device that is not on the home network yet", () => {
  it("hands every wrap to the carrier, then reports 'not sent yet' naming the queued device", async () => {
    // Robin's laptop is away from home (queued); Mia's phone is connected.
    const send = installBridge((r) => (r === DPK1 ? "queued" : "sent"));
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const { signer, routed } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }, { id: "d2", pubkey: DPK2 }], relays: RELAYS },
      relay,
    );
    await signer.connect("local");

    const err = await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect((err as Error).message).toBe("Not sent yet — it'll reach Robin's laptop when it's on the home network.");
    // The queued device did not stop the other device's wrap going out.
    expect(send.mock.calls.map((c) => c[0])).toEqual([DPK1, DPK2]);
    expect(routed.map((r) => r.meta.kind)).toEqual(["clause", "clause"]);
  });

  it("a genuine relay failure with no local hold still aborts with the relay error", async () => {
    installBridge(() => "rejected");
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const { signer } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }, { id: "d2", pubkey: DPK2 }], relays: RELAYS },
      relay,
    );
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow("Couldn't reach any relay");
    expect(relay).toHaveBeenCalledTimes(1);
  });
});

describe("tracking metadata", () => {
  it("tags a grant with the ask it answers, so its ACK can be tied back", async () => {
    installBridge(() => "sent");
    const reqId = "ab".repeat(32);
    const { signer, routed } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: RELAYS },
      async () => {},
    );
    await signer.connect("local");
    const ctx: DecisionContext = {
      childId: "c",
      minutesGranted: 20,
      tz: "UTC",
      wire: { reqId, nonce: "cd".repeat(32), machine: DPK1, subject: SUBJECT, limitHit: "budget" },
    };
    await signer.signDecision("req_1", "approved", ctx);
    expect(routed).toHaveLength(1);
    expect(routed[0].machine).toBe(DPK1);
    expect(routed[0].meta).toEqual({ kind: "grant", reqId });
  });

  it("a grant held for a ward not at home leaves the ask pending (the error propagates)", async () => {
    installBridge(() => "queued");
    const { signer } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: RELAYS },
      async () => {
        throw new Error("Couldn't reach any relay — check your connection and try again.");
      },
    );
    await signer.connect("local");
    const ctx: DecisionContext = {
      childId: "c",
      minutesGranted: 20,
      tz: "UTC",
      wire: { reqId: "ab".repeat(32), nonce: "cd".repeat(32), machine: DPK1, subject: SUBJECT, limitHit: "budget" },
    };
    await expect(signer.signDecision("req_1", "approved", ctx)).rejects.toBeInstanceOf(QueuedForHomeNetworkError);
  });
});
