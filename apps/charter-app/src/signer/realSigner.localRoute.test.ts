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

function installBridge(
  sendFor: (recipient: string) => string,
  // What the ward makes of a wrap the carrier SENT it (default: its receipt).
  wardFor: (recipient: string) => string = () => "received",
) {
  // A faithful little carrier: a wrap it sent is soon received by the ward; one
  // it queued stays queued.
  const stateOf = new Map<string, string>();
  const send = vi.fn((recipient: string, wrapJson: string) => {
    const state = sendFor(recipient);
    stateOf.set((JSON.parse(wrapJson) as { id: string }).id, state === "sent" ? wardFor(recipient) : state);
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
    deviceName: (m) => names[m],
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
    const r = await signer.signGiftClause!("c", 15, "Europe/London");
    expect(r).toEqual({ ok: true, wraps: [expect.any(String)] }); // the wrap the carrier took
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
    // ...and the error says the other device DID get it, by machine key.
    expect((err as QueuedForHomeNetworkError).devices).toEqual(["Robin's laptop"]);
    expect((err as QueuedForHomeNetworkError).reached).toEqual([DPK2]);
  });

  it("a device whose every wrap was held is never reported as reached", async () => {
    installBridge(() => "queued");
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const { signer } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }, { id: "d2", pubkey: DPK2 }], relays: RELAYS },
      relay,
    );
    await signer.connect("local");
    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect(err.devices).toEqual(["Robin's laptop", "Mia's phone"]);
    expect(err.reached).toEqual([]);
  });

  // Amended rule (step 5 of "Routing and fallback"): one device's failure never
  // abandons the others, so BOTH devices are attempted before the relay error
  // is thrown — it used to stop after the first.
  it("a genuine relay failure with no local hold still throws the relay error, after trying every device", async () => {
    installBridge(() => "rejected");
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const { signer } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }, { id: "d2", pubkey: DPK2 }], relays: RELAYS },
      relay,
    );
    await signer.connect("local");
    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as Error;
    expect(err).not.toBeInstanceOf(QueuedForHomeNetworkError);
    expect(err.message).toBe(
      "Not sent to Robin's laptop and Mia's phone: Couldn't reach any relay — check your connection and try again.",
    );
    expect(relay).toHaveBeenCalledTimes(2);
  });

  it("a lone device's relay failure keeps the relay error word for word", async () => {
    installBridge(() => "rejected");
    const failure = new Error("Couldn't reach any relay — check your connection and try again.");
    const { signer } = makeSigner(
      { subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: RELAYS },
      async () => {
        throw failure;
      },
    );
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toBe(failure);
  });
});

const RELAY_DOWN = "Couldn't reach any relay — check your connection and try again.";
const relayDown = () =>
  vi.fn(async () => {
    throw new Error(RELAY_DOWN);
  });
const TWO = (): ChildTarget => ({
  subject: SUBJECT,
  devices: [
    { id: "d1", pubkey: DPK1 },
    { id: "d2", pubkey: DPK2 },
  ],
  relays: RELAYS,
});

describe("H2: one device's failure never abandons the others", () => {
  it("probe A: a held device and a device the carrier won't take — the hold is reported, not lost", async () => {
    const send = installBridge((r) => (r === DPK1 ? "queued" : "rejected"));
    const relay = relayDown();
    const { signer } = makeSigner(TWO(), relay);
    await signer.connect("local");

    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect(send.mock.calls.map((c) => c[0])).toEqual([DPK1, DPK2]); // both tried
    expect(relay).toHaveBeenCalledTimes(2);
    expect(err.devices).toEqual(["Robin's laptop"]);
    expect(err.reached).toEqual([]);
    expect(err.failed).toEqual([{ machine: DPK2, error: RELAY_DOWN }]);
    expect(err.wraps).toHaveLength(1); // the held wrap, for tracking
  });

  it("probe A2: a device that got it and a device that didn't — a partial result, never a bare relay error", async () => {
    installBridge((r) => (r === DPK1 ? "sent" : "rejected"));
    const { signer } = makeSigner(TWO(), relayDown());
    await signer.connect("local");

    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect(err.reached).toEqual([DPK1]);
    expect(err.devices).toEqual([]);
    expect(err.failed).toEqual([{ machine: DPK2, error: RELAY_DOWN }]);
  });

  it("the device that failed first does not stop a later one being reached", async () => {
    const send = installBridge((r) => (r === DPK1 ? "rejected" : "sent"));
    const { signer } = makeSigner(TWO(), relayDown());
    await signer.connect("local");
    const err = (await signer.signStandDownClause!("c").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(send).toHaveBeenCalledTimes(2);
    expect(err.reached).toEqual([DPK2]);
    expect(err.failed.map((f) => f.machine)).toEqual([DPK1]);
  });

  it("every device reached resolves, with the wraps the carrier took", async () => {
    installBridge(() => "sent");
    const { signer } = makeSigner(TWO(), relayDown());
    await signer.connect("local");
    const r = await signer.signMaintenanceClause!("c", 10);
    expect(r.ok).toBe(true);
    expect(r.wraps).toHaveLength(2);
  });

  it("names each device's own state when nothing reached anywhere", async () => {
    installBridge((r) => (r === DPK1 ? "sent" : "rejected"), () => "refused");
    const { signer } = makeSigner(TWO(), relayDown());
    await signer.connect("local");
    await expect(signer.signUpdateClause("c", { versionCode: 9, versionName: "9.0" } as never, "https://x.example/a.apk")).rejects.toThrow(
      `Robin's laptop didn't accept this change. Not sent to Mia's phone: ${RELAY_DOWN}`,
    );
  });
});

describe("M2: a problem ACK is never 'reached'", () => {
  it("probe C: a lone ward that refused it within the window is a failure, not success", async () => {
    installBridge(() => "sent", () => "refused");
    const { signer } = makeSigner({ subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: RELAYS }, relayDown());
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow(
      "Robin's laptop didn't accept this change.",
    );
  });

  it.each([
    ["failed", "Robin's laptop couldn't carry this out. It may need another try."],
    ["expired", "Robin's laptop didn't act on this: it had already expired there."],
    ["superseded", "Robin's laptop already had a newer change, so it didn't use this one."],
  ])("an ACK of '%s' is reported in plain words", async (ack, words) => {
    installBridge(() => "sent", () => ack);
    const { signer } = makeSigner({ subject: SUBJECT, devices: [{ id: "d1", pubkey: DPK1 }], relays: RELAYS }, async () => {});
    await signer.connect("local");
    await expect(signer.signGiftClause!("c", 15, "Europe/London")).rejects.toThrow(words);
  });

  it("a refusing device beside a held one is named as failed, not as reached", async () => {
    installBridge((r) => (r === DPK1 ? "sent" : "queued"), () => "refused");
    const { signer } = makeSigner(TWO(), relayDown());
    await signer.connect("local");
    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect(err.reached).toEqual([]);
    expect(err.devices).toEqual(["Mia's phone"]);
    expect(err.failed).toEqual([{ machine: DPK1, ack: "refused" }]);
  });

  it("a refusing device beside one that applied it is a partial result", async () => {
    installBridge(() => "sent", (r) => (r === DPK1 ? "applied" : "refused"));
    const { signer } = makeSigner(TWO(), async () => {});
    await signer.connect("local");
    const err = (await signer.signGiftClause!("c", 15, "Europe/London").catch((e: unknown) => e)) as QueuedForHomeNetworkError;
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect(err.reached).toEqual([DPK1]);
    expect(err.failed).toEqual([{ machine: DPK2, ack: "refused" }]);
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

  it("a grant held for a ward not at home propagates as 'held' (the store records the ask as answered)", async () => {
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
