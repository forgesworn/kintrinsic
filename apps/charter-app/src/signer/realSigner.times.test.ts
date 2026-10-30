// Times (contract §Times) on the signer: typed gifts to chosen devices, the
// `time` echo on a granted ask, and answering an ask with a different kind of
// time (a 0-minute closing grant plus a gift, both to the asking device only).

import { beforeEach, describe, expect, it } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import { unwrapClause, unwrapGrant, type GuardianOps } from "../wire/giftwrap";
import type { GrantGift } from "../wire/types";
import type { DecisionContext } from "./Signer";
import { RealSigner, type ChildTarget } from "./realSigner";
import { QueuedForHomeNetworkError } from "./routePublish";
import { resolveChildTarget } from "./resolveChild";
import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";

beforeEach(() => localStorage.clear());

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

const DEV1 = generateSecretKey();
const DEV2 = generateSecretKey();
const DPK1 = getPublicKey(DEV1);
const DPK2 = getPublicKey(DEV2);
const SUBJECT = getPublicKey(generateSecretKey());
const RELAYS = ["wss://relay.one"];
const TARGET: ChildTarget = {
  subject: SUBJECT,
  devices: [
    { id: "laptop", pubkey: DPK1 },
    { id: "phone", pubkey: DPK2 },
  ],
  relays: RELAYS,
};

function setup(target: ChildTarget = TARGET, confirmGate?: () => boolean) {
  const published: { relays: string[]; event: NostrEvent }[] = [];
  const signer = new RealSigner({
    connectGuardian: async () => localGuardian(),
    publish: async (relays, event) => {
      published.push({ relays, event });
    },
    resolveChild: () => target,
    now: () => 1_700_000_000,
    confirmGate,
  });
  return { signer, published };
}

/** The gift (if any) a given device can open among the published wraps. */
function giftFor(published: { event: NostrEvent }[], sk: Uint8Array): GrantGift | null {
  for (const p of published) {
    try {
      const u = unwrapClause(p.event, sk, GUARDIAN);
      if (u.payload.kind === "gift") return u.payload.body as GrantGift;
    } catch {
      // sealed to someone else
    }
  }
  return null;
}

describe("signGiftClause — typed gifts to chosen devices", () => {
  it("with no options goes to every device and carries no time (unchanged)", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await signer.signGiftClause!("c", 30, "Europe/London");
    expect(published).toHaveLength(2);
    const body = giftFor(published, DEV1) as GrantGift;
    expect("time" in body).toBe(false);
    expect("groupId" in body).toBe(false);
  });

  it("wraps ONLY to the listed devices and carries the time", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await signer.signGiftClause!("c", 45, "Europe/London", undefined, {
      time: "learning",
      deviceIds: ["laptop"],
    });
    expect(published).toHaveLength(1);
    expect(giftFor(published, DEV1)).toMatchObject({ minutes: 45, time: "learning" });
    // The phone cannot open it: it was never sealed to it.
    expect(giftFor(published, DEV2)).toBeNull();
  });

  it("lists in any order and several devices at once", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await signer.signGiftClause!("c", 10, "UTC", undefined, { deviceIds: ["phone", "laptop"] });
    expect(published).toHaveLength(2);
  });

  it("treats 'free' as Free time: no time key at all", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await signer.signGiftClause!("c", 10, "UTC", undefined, { time: "free", deviceIds: ["laptop"] });
    expect("time" in (giftFor(published, DEV1) as GrantGift)).toBe(false);
  });

  it("refuses an empty device list rather than reading it as everyone", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await expect(
      signer.signGiftClause!("c", 10, "UTC", undefined, { deviceIds: [] }),
    ).rejects.toThrow(/choose which device/i);
    expect(published).toHaveLength(0);
  });

  it("refuses a device that is not one of the child's, sending nothing", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await expect(
      signer.signGiftClause!("c", 10, "UTC", undefined, { deviceIds: ["laptop", "nope"] }),
    ).rejects.toThrow(/isn't paired/i);
    expect(published).toHaveLength(0);
  });

  it("refuses a malformed time id, and a gift that names both a group and a time", async () => {
    const { signer, published } = setup();
    await signer.connect("signet");
    await expect(
      signer.signGiftClause!("c", 10, "UTC", undefined, { time: "Not An Id" }),
    ).rejects.toThrow(/isn't a kind of time/i);
    await expect(
      signer.signGiftClause!("c", 10, "UTC", "play", { time: "learning" }),
    ).rejects.toThrow(/not both/i);
    expect(published).toHaveLength(0);
  });

  it("names the time and the devices in the confirm sheet", async () => {
    const seen: string[] = [];
    const published: { event: NostrEvent }[] = [];
    const signer = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async (_relays: string[], event: NostrEvent) => {
        published.push({ event });
      },
      resolveChild: () => TARGET,
      now: () => 1_700_000_000,
      confirmGate: (info) => {
        seen.push(info.detail);
        return true;
      },
      deviceName: (m) => (m === DPK1 ? "Sam's laptop" : undefined),
    });
    await signer.connect("local");
    await signer.signGiftClause!("c", 20, "UTC", undefined, {
      time: "learning",
      timeLabel: "Learning time",
      deviceIds: ["laptop"],
    });
    expect(seen.join("\n")).toMatch(/20 more minutes of Learning time/);
    expect(seen.join("\n")).toMatch(/Sam's laptop/);
  });
});

const REQ_ID = "ab".repeat(32);
const NONCE = "cd".repeat(32);
function askCtx(over: Partial<DecisionContext> = {}): DecisionContext {
  return {
    childId: "c",
    minutesGranted: 20,
    tz: "UTC",
    wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "budget" },
    ...over,
  };
}

describe("signDecision — the time echo", () => {
  it("echoes the request's time verbatim on allow", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");
    await signer.signDecision(
      "r",
      "approved",
      askCtx({
        wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "budget", time: "learning" },
      }),
    );
    const g = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(g.payload.params).toEqual({ minutesGranted: 20, limitHit: "budget", time: "learning" });
  });

  it("echoes it on deny too, and leaves it out when the ask had none", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");
    await signer.signDecision(
      "r",
      "denied",
      askCtx({
        minutesGranted: 0,
        wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "schedule", time: "learning" },
      }),
    );
    expect(unwrapGrant(published[0].event, DEV1, GUARDIAN).payload.params).toEqual({
      minutesGranted: 0,
      limitHit: "schedule",
      time: "learning",
    });
    published.length = 0;
    await signer.signDecision("r", "approved", askCtx());
    expect("time" in unwrapGrant(published[0].event, DEV1, GUARDIAN).payload.params).toBe(false);
  });
});

describe("answerAskWithOtherTime", () => {
  it("sends a 0-minute ALLOW grant plus a typed gift, both to the asking device only", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");

    await signer.answerAskWithOtherTime!("r", askCtx(), 30, "learning");

    expect(published).toHaveLength(2);
    // The asker is DEV1; DEV2 can open neither wrap.
    for (const p of published) {
      expect(() => unwrapGrant(p.event, DEV2, GUARDIAN)).toThrow();
    }
    const grant = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(grant.valid).toBe(true);
    expect(grant.payload).toMatchObject({
      op: "time.extend",
      reqId: REQ_ID,
      nonce: NONCE,
      // allow, not deny: a deny would tell the child "not right now".
      decision: "allow",
      params: { minutesGranted: 0, limitHit: "budget" },
    });
    expect(giftFor(published, DEV1)).toMatchObject({ minutes: 30, time: "learning" });
    expect(giftFor(published, DEV2)).toBeNull();
  });

  it("still echoes the REQUEST's own time in the closing grant while buying another", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");
    await signer.answerAskWithOtherTime!(
      "r",
      askCtx({
        wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "budget", time: "learning" },
      }),
      30,
      undefined, // ... Free time instead
    );
    expect(unwrapGrant(published[0].event, DEV1, GUARDIAN).payload.params).toEqual({
      minutesGranted: 0,
      limitHit: "budget",
      time: "learning",
    });
    expect("time" in (giftFor(published, DEV1) as GrantGift)).toBe(false);
  });

  it("is one confirm for both wraps", async () => {
    let asked = 0;
    const { signer } = setup(TARGET, () => {
      asked += 1;
      return true;
    });
    // Not the local signer: there the answer press is itself the confirmation
    // and `decision` skips the sheet. Signet-style signers gate it, once.
    await signer.connect("signet");
    await signer.answerAskWithOtherTime!("r", askCtx(), 30, "learning");
    expect(asked).toBe(1);
  });

  it("sends nothing when the guardian cancels the confirm", async () => {
    const { signer, published } = setup(TARGET, () => false);
    await signer.connect("signet");
    await expect(signer.answerAskWithOtherTime!("r", askCtx(), 30, "learning")).rejects.toThrow("cancelled");
    expect(published).toHaveLength(0);
  });

  it("refuses an ask with no wire correlation, an unknown asker, or a bad time id", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");
    await expect(
      signer.answerAskWithOtherTime!("r", askCtx({ wire: undefined }), 30, "learning"),
    ).rejects.toThrow(/came from a device/i);
    await expect(
      signer.answerAskWithOtherTime!(
        "r",
        askCtx({ wire: { reqId: REQ_ID, nonce: NONCE, machine: "ee".repeat(32), subject: SUBJECT, limitHit: "budget" } }),
        30,
        "learning",
      ),
    ).rejects.toThrow(/isn't paired/i);
    await expect(signer.answerAskWithOtherTime!("r", askCtx(), 30, "NOPE")).rejects.toThrow(
      /isn't a kind of time/i,
    );
    expect(published).toHaveLength(0);
  });

  it("does not send the gift if the closing grant cannot be built (no tz)", async () => {
    const { signer, published } = setup(TARGET, () => true);
    await signer.connect("local");
    await expect(
      signer.answerAskWithOtherTime!("r", askCtx({ tz: undefined }), 30, "learning"),
    ).rejects.toThrow(/time zone is unknown/i);
    expect(published).toHaveLength(0);
  });
});

describe("answerAskWithOtherTime — delivery", () => {
  /** A signer whose publish rejects the GRANT as the carrier would for a ward
   *  that is away from the home network, and records everything it was given. */
  function withGrantFailure(make: () => Error) {
    const seen: { kind?: string; event: NostrEvent }[] = [];
    const signer = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async (_relays: string[], event: NostrEvent, meta?: { kind: string }) => {
        seen.push({ kind: meta?.kind, event });
        if (meta?.kind === "grant") throw make();
      },
      resolveChild: () => TARGET,
      now: () => 1_700_000_000,
      confirmGate: () => true,
    });
    return { signer, seen };
  }

  it("a grant held for the home network does not stop the gift, and the answer is reported as held", async () => {
    const { signer, seen } = withGrantFailure(() => new QueuedForHomeNetworkError(["Sam's laptop"]));
    await signer.connect("signet");
    await expect(signer.answerAskWithOtherTime!("r", askCtx(), 30, "learning")).rejects.toBeInstanceOf(
      QueuedForHomeNetworkError,
    );
    // The gift still went out behind the held grant ...
    const gifts = seen.filter((p) => p.kind === "clause");
    expect(gifts).toHaveLength(1);
    expect(giftFor(gifts, DEV1)).toMatchObject({ minutes: 30, time: "learning" });
    // ... and nothing reached the sibling.
    expect(seen.every((p) => { try { unwrapGrant(p.event, DEV2, GUARDIAN); return false; } catch { return true; } })).toBe(true);
  });

  it("a grant that genuinely fails stops BEFORE any minutes are given", async () => {
    const { signer, seen } = withGrantFailure(() => new Error("relay said no"));
    await signer.connect("signet");
    await expect(signer.answerAskWithOtherTime!("r", askCtx(), 30, "learning")).rejects.toThrow("relay said no");
    expect(seen.filter((p) => p.kind === "clause")).toHaveLength(0);
  });
});

describe("typed gifts and answers refuse a device that cannot run times", () => {
  const OLD: ChildTarget = {
    subject: SUBJECT,
    devices: [
      { id: "laptop", pubkey: DPK1, timesCapable: true },
      { id: "phone", pubkey: DPK2, timesCapable: false },
    ],
    relays: RELAYS,
  };

  it("refuses a typed gift to it, sending nothing; a Free gift still goes", async () => {
    const { signer, published } = setup(OLD);
    await signer.connect("signet");
    await expect(
      signer.signGiftClause!("c", 10, "UTC", undefined, { time: "learning", deviceIds: ["laptop", "phone"] }),
    ).rejects.toThrow(/can't run this kind of time/i);
    expect(published).toHaveLength(0);
    await signer.signGiftClause!("c", 10, "UTC", undefined, { deviceIds: ["phone"] });
    expect(published).toHaveLength(1);
    await signer.signGiftClause!("c", 10, "UTC", undefined, { time: "learning", deviceIds: ["laptop"] });
    expect(published).toHaveLength(2);
  });

  it("refuses to re-type an ask from it", async () => {
    const { signer, published } = setup(OLD, () => true);
    await signer.connect("signet");
    await expect(
      signer.answerAskWithOtherTime!("r", askCtx({ wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK2, subject: SUBJECT, limitHit: "budget" } }), 30, "learning"),
    ).rejects.toThrow(/can't run this kind of time/i);
    expect(published).toHaveLength(0);
  });

  it("resolveChildTarget stamps each device from its live STATUS (never-reported = not capable)", () => {
    const child = {
      id: "c",
      name: "Sam",
      color: "#000",
      dependantPubkey: SUBJECT,
      devices: [
        { id: "laptop", label: "L", platform: "linux", pairing: "paired", devicePubkey: DPK1 },
        { id: "phone", label: "P", platform: "android", pairing: "paired", devicePubkey: DPK2 },
      ],
      policies: [],
    } as unknown as Child;
    const st = { [DPK1]: { appVersionCode: 800 } as DeviceStatus };
    const t = resolveChildTarget([child], "c", RELAYS, st)!;
    expect(t.devices.map((d) => [d.id, d.timesCapable])).toEqual([["laptop", true], ["phone", false]]);
    // Without a status map nothing is stamped (demo and tests).
    expect(resolveChildTarget([child], "c", RELAYS)!.devices.every((d) => !("timesCapable" in d))).toBe(true);
  });
});
