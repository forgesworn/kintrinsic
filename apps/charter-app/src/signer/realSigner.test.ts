import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import type { Policy } from "../domain/types";
import { appRulesToGrant } from "../wire/clause";
import type { GrantAppRules } from "../wire/types";
import { endOfDayUnix } from "../wire/grant";
import { unwrapClause, unwrapGrant, type GuardianOps } from "../wire/giftwrap";
import type { DecisionContext } from "./Signer";
import { RealSigner, type ChildTarget, type SignedRule } from "./realSigner";

// The signer persists its clause clock (`nextIssuedAt`) so a reload can't step
// back under it; with a frozen `now` that floor would leak between tests.
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
const RELAYS = ["wss://relay.one", "wss://relay.two"];

const devicePolicy: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: { tz: "Europe/London", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
  budget: { tz: "Europe/London", dailyMinutes: 90 },
};

function makeSigner(
  target: ChildTarget | undefined,
  published: { relays: string[]; event: NostrEvent }[],
  opts: { confirmGate?: () => boolean } = {},
) {
  return new RealSigner({
    connectGuardian: async () => localGuardian(),
    publish: async (relays, event) => {
      published.push({ relays, event });
    },
    resolveChild: () => target,
    now: () => 1_700_000_000,
    confirmGate: opts.confirmGate,
  });
}

describe("RealSigner.signRelayConfig", () => {
  it("creates an offline code bound to the device without publishing", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(undefined, published);
    await signer.connect("local");
    const event = JSON.parse(await signer.signRelayConfig(DPK1, RELAYS, "Robin’s laptop")) as NostrEvent;
    expect(event.pubkey).toBe(GUARDIAN);
    expect(event.kind).toBe(31118);
    expect(event.tags).toContainEqual(["p", DPK1]);
    expect(JSON.parse(event.content)).toEqual({ machine: DPK1, relays: RELAYS });
    expect(published).toEqual([]);
  });

  it("authorises battery settings for exactly the requested device", async () => {
    const { recommendedBattery } = await import("../domain/battery");
    const gate = vi.fn(() => true);
    const signer = makeSigner(undefined, [], { confirmGate: gate });
    await signer.connect("signet");
    const event = JSON.parse(await signer.signRelayConfig(DPK2, RELAYS, "Ward phone", recommendedBattery)) as NostrEvent;
    expect(gate).toHaveBeenCalled();
    expect(event.tags).toContainEqual(["p", DPK2]);
    expect(event.tags).not.toContainEqual(["p", DPK1]);
    expect(JSON.parse(event.content).battery).toEqual(recommendedBattery);
  });

  it("honours a cancelled guardian authorisation", async () => {
    const signer = makeSigner(undefined, [], { confirmGate: () => false });
    await signer.connect("local");
    await expect(signer.signRelayConfig(DPK1, RELAYS, "Robin’s laptop")).rejects.toThrow("cancelled");
  });
});

describe("RealSigner.signClause", () => {
  it("gift-wraps every clause to every device and publishes to the child's relays", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS }, published);
    await signer.connect("signet");

    await signer.signClause("child_sam", devicePolicy);

    // 2 clauses (schedule + budget) × 2 devices = 4 published 1059s.
    expect(published).toHaveLength(4);
    for (const p of published) {
      expect(p.relays).toEqual(RELAYS);
      expect(p.event.kind).toBe(1059);
    }
    // Device 1 receives both kinds, correctly addressed + signed by the guardian.
    const forDev1 = published
      .map((p) => {
        try {
          return unwrapClause(p.event, DEV1, GUARDIAN);
        } catch {
          return null;
        }
      })
      .filter((u): u is NonNullable<typeof u> => u !== null);
    expect(forDev1).toHaveLength(2);
    expect(forDev1.every((u) => u.valid && u.payload.subject === SUBJECT)).toBe(true);
    expect(forDev1.map((u) => u.payload.kind).sort()).toEqual(["budget", "schedule"]);
  });

  /**
   * Half A: one charter, split where it fits the child. Each device must
   * receive the charter as it applies to IT — and, just as importantly, must
   * NOT learn its sibling's rules. This is the load-bearing test for
   * per-device rules: everything else is UI on top of it.
   */
  it("sends each device its own effective rules when a control is split", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      {
        subject: SUBJECT,
        devices: [
          { id: "laptop", pubkey: DPK1 },
          { id: "phone", pubkey: DPK2 },
        ],
        relays: RELAYS,
      },
      published,
    );
    await signer.connect("signet");

    await signer.signClause("child_sam", {
      ...devicePolicy,
      deviceOverrides: {
        phone: { budget: { tz: "Europe/London", dailyMinutes: 30 } },
      },
    });

    const readFor = (dev: Uint8Array) =>
      published
        .map((p) => {
          try {
            return unwrapClause(p.event, dev, GUARDIAN);
          } catch {
            return null;
          }
        })
        .filter((u): u is NonNullable<typeof u> => u !== null);

    const laptop = readFor(DEV1);
    const phone = readFor(DEV2);

    // The laptop keeps the base charter…
    const laptopBudget = laptop.find((u) => u.payload.kind === "budget");
    expect((laptopBudget?.payload.body as { dailyMinutes: number }).dailyMinutes).toBe(90);
    // …while the phone gets its own.
    const phoneBudget = phone.find((u) => u.payload.kind === "budget");
    expect((phoneBudget?.payload.body as { dailyMinutes: number }).dailyMinutes).toBe(30);

    // The un-split control stays identical on both.
    expect(laptop.find((u) => u.payload.kind === "schedule")?.payload.body).toEqual(
      phone.find((u) => u.payload.kind === "schedule")?.payload.body,
    );

    // Nothing on the wire ever carries the override map — a device must never
    // be able to read what its sibling is allowed.
    for (const u of [...laptop, ...phone]) {
      expect(JSON.stringify(u.payload)).not.toContain("deviceOverrides");
      expect(JSON.stringify(u.payload)).not.toContain("laptop");
    }
  });

  it("requires a connection", async () => {
    const signer = makeSigner({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS }, []);
    await expect(signer.signClause("child_sam", devicePolicy)).rejects.toThrow();
  });

  it("connect sets state; setAutoSign toggles it", async () => {
    const signer = makeSigner({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS }, []);
    expect(signer.status().connected).toBe(false);
    const st = await signer.connect("signet");
    expect(st).toMatchObject({ connected: true, kind: "signet" });
    await signer.setAutoSign(true);
    expect(signer.status().autoSign).toBe(true);
    await signer.disconnect();
    expect(signer.status().connected).toBe(false);
  });

  it("publishes nothing for a child with no devices (cannot deliver)", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: SUBJECT, devices: [], relays: RELAYS }, published);
    await signer.connect("signet");
    await signer.signClause("child_sam", devicePolicy);
    expect(published).toHaveLength(0);
  });

  it("omits subject for an unbound child (single-child default)", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: null, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS }, published);
    await signer.connect("signet");
    await signer.signClause("child_sam", devicePolicy);
    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.payload.subject).toBeUndefined();
  });
});

// --- app-scope emission: ALL of a child's app policies → ONE appRules clause ---

const AT = 1_700_000_000;

const appPolicy = (over: Partial<Policy> = {}): Policy => ({
  id: "pa1",
  scope: { kind: "app", appId: "com.example.game", label: "Game" },
  ...over,
});

const chatPolicy = appPolicy({
  id: "pa2",
  scope: { kind: "app", appId: "com.example.chat", label: "Chat" },
  blocked: true,
});

/** A RealSigner that reports `childPolicies` — the store's app-scope wiring. */
function makeAppSigner(
  target: ChildTarget | undefined,
  childPolicies: Policy[],
  published: { relays: string[]; event: NostrEvent }[],
) {
  return new RealSigner({
    connectGuardian: async () => localGuardian(),
    publish: async (relays, event) => {
      published.push({ relays, event });
    },
    resolveChild: () => target,
    childPolicies: () => childPolicies,
    now: () => AT,
  });
}

describe("RealSigner.signClause app-scope (appRules aggregation)", () => {
  it("emits ONE appRules clause aggregating every app policy, merging the saved one", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    // State still holds the STALE pa1 (blocked:false) alongside pa2 and a
    // device policy the aggregation must ignore.
    const inState: Policy[] = [appPolicy({ blocked: false }), chatPolicy, devicePolicy];
    const signer = makeAppSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      inState,
      published,
    );
    await signer.connect("signet");

    const saved = appPolicy({ blocked: true }); // the just-toggled edit
    await signer.signClause("child_sam", saved);

    // Exactly one CLAUSE (one clause × one device).
    expect(published).toHaveLength(1);
    expect(published[0].relays).toEqual(RELAYS);
    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.valid).toBe(true);
    expect(u.payload.kind).toBe("apprules");
    expect(u.payload.subject).toBe(SUBJECT);
    // Body == appRulesToGrant over [others-without-pa1, saved] (device dropped).
    expect(u.payload.body).toEqual(appRulesToGrant([chatPolicy, saved], AT));
    // The saved pa1 carries blocked:true (merged over the stale false).
    const rules = (u.payload.body as GrantAppRules).rules;
    expect(rules.find((r) => r.pkg === "com.example.game")!.blocked).toBe(true);
    expect(rules.map((r) => r.pkg)).toEqual(["com.example.chat", "com.example.game"]);
  });

  it("carries a per-app allowed-hours schedule via scheduleToGrant", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const saved = appPolicy({
      schedule: { tz: "Europe/London", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
    });
    const signer = makeAppSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      [saved],
      published,
    );
    await signer.connect("signet");
    await signer.signClause("child_sam", saved);

    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.payload.body).toEqual(appRulesToGrant([saved], AT));
    expect((u.payload.body as GrantAppRules).rules[0].schedule?.weekly.mon).toEqual([
      { start: "16:00", end: "18:00" },
    ]);
  });

  it("a lone app policy yields a one-rule set; no other app policies present", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const saved = appPolicy({ blocked: true });
    const signer = makeAppSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      [saved],
      published,
    );
    await signer.connect("signet");
    await signer.signClause("child_sam", saved);

    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.payload.body).toEqual({
      v: 1,
      issuedAt: AT,
      rules: [{ pkg: "com.example.game", label: "Game", blocked: true }],
    });
  });

  it("publishes nothing for an app save when the child has no paired devices", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeAppSigner(
      { subject: SUBJECT, devices: [], relays: RELAYS },
      [appPolicy()],
      published,
    );
    await signer.connect("signet");
    await signer.signClause("child_sam", appPolicy());
    expect(published).toHaveLength(0);
  });

  it("a device-scope save is UNCHANGED — schedule + budget, never apprules", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    // childPolicies is wired, but a device save must still ignore it entirely.
    const signer = makeAppSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      [appPolicy({ blocked: true })],
      published,
    );
    await signer.connect("signet");
    await signer.signClause("child_sam", devicePolicy);

    expect(published).toHaveLength(2); // schedule + budget, one device
    const kinds = published
      .map((p) => unwrapClause(p.event, DEV1, GUARDIAN).payload.kind)
      .sort();
    expect(kinds).toEqual(["budget", "schedule"]);
  });

  // 2026-08-04: the decision-skip added to `authorize()` (see the
  // RealSigner.signDecision describe below) must never leak into a clause
  // save — a rule edit stays gated with "nothing changes until you confirm",
  // local signer included.
  it("still routes a clause save through the confirm gate on the LOCAL signer", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const gate = vi.fn(() => true);
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: gate },
    );
    await signer.connect("local");

    await signer.signClause("child_sam", devicePolicy);

    expect(gate).toHaveBeenCalledWith(
      expect.objectContaining({ action: "clause", decision: undefined, signerKind: "local" }),
    );
  });

  it("a cancelled confirm gate still blocks a clause save on the local signer", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: () => false },
    );
    await signer.connect("local");

    await expect(signer.signClause("child_sam", devicePolicy)).rejects.toThrow("cancelled");
    expect(published).toHaveLength(0);
  });
});

const REQ_ID = "ab".repeat(32);
const NONCE = "cd".repeat(32);

/** A real relay-ingested ask on DEVICE 1 — what the store threads through. */
function wireCtx(over: Partial<DecisionContext> = {}): DecisionContext {
  return {
    childId: "child_sam",
    minutesGranted: 20,
    tz: "UTC",
    wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "budget" },
    ...over,
  };
}

describe("RealSigner.signDecision", () => {
  it("approve publishes ONE guardian-signed GRANT wrapped to the asking machine", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    await signer.signDecision("req_1", "approved", wireCtx());

    // Exactly one wrap — to the machine that asked, never fanned to DPK2.
    expect(published).toHaveLength(1);
    expect(published[0].relays).toEqual(RELAYS);
    const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(got.valid).toBe(true);
    expect(got.payload).toMatchObject({
      v: 1,
      op: "time.extend",
      reqId: REQ_ID, // echoed verbatim
      nonce: NONCE, // echoed verbatim
      decision: "allow",
      ts: 1_700_000_000,
      params: { minutesGranted: 20, limitHit: "budget" },
    });
    expect(got.payload.exp).toBeGreaterThan(got.payload.ts);
    // DEVICE 2 cannot open it (it wasn't the asker).
    expect(() => unwrapGrant(published[0].event, DEV2, GUARDIAN)).toThrow();
  });

  it("deny publishes a deny GRANT carrying 0 minutes", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    await signer.signDecision("req_1", "denied", wireCtx({ minutesGranted: 0 }));

    const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(got.valid).toBe(true);
    expect(got.payload.decision).toBe("deny");
    expect(got.payload.params).toEqual({ minutesGranted: 0, limitHit: "budget" });
  });

  it("install.apk approve publishes a GRANT pinning the catalog cert to the asking machine", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    const cert = "3c".repeat(32);
    await signer.signDecision("req_1", "approved", {
      childId: "child_sam",
      install: {
        reqId: REQ_ID,
        nonce: NONCE,
        machine: DPK1,
        packageName: "app.example.thing",
        signerCertSha256: cert,
        source: "staged",
      },
    });

    expect(published).toHaveLength(1);
    const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(got.valid).toBe(true);
    expect(got.payload).toMatchObject({
      v: 1,
      op: "install.apk",
      reqId: REQ_ID,
      nonce: NONCE,
      decision: "allow",
      params: { packageName: "app.example.thing", signerCertSha256: cert, source: "staged" },
    });
    expect(got.payload.exp).toBeGreaterThan(got.payload.ts);
    // Only the asking device can open it.
    expect(() => unwrapGrant(published[0].event, DEV2, GUARDIAN)).toThrow();
  });

  it("install.apk deny publishes a signed deny GRANT even with no pinned cert (uncurated)", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    // No signerCertSha256 — the app isn't curated, but a refusal still travels.
    await signer.signDecision("req_1", "denied", {
      childId: "child_sam",
      install: {
        reqId: REQ_ID,
        nonce: NONCE,
        machine: DPK1,
        packageName: "app.uncurated.thing",
        source: "staged",
      },
    });

    expect(published).toHaveLength(1);
    const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
    expect(got.valid).toBe(true);
    expect(got.payload).toMatchObject({
      op: "install.apk",
      reqId: REQ_ID,
      nonce: NONCE,
      decision: "deny",
      params: { packageName: "app.uncurated.thing", source: "staged" },
    });
    // The deny carries the zero sentinel, never a fabricated real digest.
    expect((got.payload.params as { signerCertSha256: string }).signerCertSha256).toBe(
      "0".repeat(64),
    );
  });

  it("a request without wire correlation (simulated/demo) approves locally, publishes NOTHING", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    await expect(
      signer.signDecision("req_1", "approved", wireCtx({ wire: undefined })),
    ).resolves.toEqual({ ok: true });
    expect(published).toHaveLength(0);
  });

  it("refuses a wire decision with no clause tz rather than emit a phone-local grant", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: () => true },
    );
    await signer.connect("local");

    await expect(
      signer.signDecision("req_1", "approved", wireCtx({ tz: undefined })),
    ).rejects.toThrow(/time zone is unknown/);
    expect(published).toHaveLength(0);
  });

  // 2026-08-04: a decision signed by the LOCAL key skips the confirm gate
  // entirely (the Approve/Not now press on the request list already IS the
  // confirmation — see `authorize()`), so a gate wired to refuse can no
  // longer cancel a local decision. This used to be titled "a cancelled
  // confirm gate signs nothing"; that guarantee now holds for a clause save
  // (see the signClause describe above) and for an externally-gated decision
  // (MockSigner is the real external path — see mockSigner.test.ts), not for
  // a local one.
  it("a local decision is never gated — a refusing confirmGate cannot cancel it", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const gate = vi.fn(() => false);
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
      { confirmGate: gate },
    );
    await signer.connect("local");

    await expect(signer.signDecision("req_1", "approved", wireCtx())).resolves.toEqual({
      ok: true,
    });
    expect(gate).not.toHaveBeenCalled();
    expect(published).toHaveLength(1);
  });

  it("the non-gated (Signet) path still refuses explicitly", async () => {
    const signer = makeSigner({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS }, []);
    await signer.connect("signet");
    await expect(signer.signDecision("req_1", "approved", wireCtx())).rejects.toThrow(
      "not implemented",
    );
  });

  // Named times: a bucket-hit ask's GRANT must echo bucketId verbatim, exactly
  // like limitHit itself — on both allow and deny.
  describe("bucketId echo (named times)", () => {
    it("echoes bucketId on an allow", async () => {
      const published: { relays: string[]; event: NostrEvent }[] = [];
      const signer = makeSigner(
        { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
        published,
        { confirmGate: () => true },
      );
      await signer.connect("local");

      await signer.signDecision(
        "req_1",
        "approved",
        wireCtx({ wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "bucket", bucketId: "play" } }),
      );

      const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
      expect(got.payload.params).toMatchObject({ limitHit: "bucket", bucketId: "play" });
    });

    it("echoes bucketId on a deny, and stays absent for a whole-device dimension", async () => {
      const published: { relays: string[]; event: NostrEvent }[] = [];
      const signer = makeSigner(
        { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
        published,
        { confirmGate: () => true },
      );
      await signer.connect("local");

      await signer.signDecision(
        "req_1",
        "denied",
        wireCtx({
          minutesGranted: 0,
          wire: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, limitHit: "bucket", bucketId: "play" },
        }),
      );
      const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
      expect(got.payload.params).toMatchObject({ bucketId: "play" });

      published.length = 0;
      await signer.signDecision("req_2", "approved", wireCtx()); // wireCtx()'s default limitHit is "budget"
      const budget = unwrapGrant(published[0].event, DEV1, GUARDIAN);
      expect("bucketId" in (budget.payload.params as object)).toBe(false);
    });
  });

  // app.open: the plain Decision echo — {pkg, minutesGranted} (fixed
  // 2026-08-03, review round 1: an empty-params grant could never verify on
  // the ward, see wire/grant.ts's buildAppOpenGrant doc). The AppHold that
  // actually opens the app is a SEPARATE clause save the store makes
  // alongside this (see store.tsx's approveAppOpen).
  describe("app.open decision echo", () => {
    const PKG = "com.mojang.minecraftpe";
    function appOpenCtx(over: Partial<DecisionContext> = {}): DecisionContext {
      return {
        childId: "child_sam",
        minutesGranted: 30,
        appOpen: { reqId: REQ_ID, nonce: NONCE, machine: DPK1, subject: SUBJECT, pkg: PKG },
        ...over,
      };
    }

    it("approve publishes an allow GRANT echoing pkg + the granted minutes to the asking machine", async () => {
      const published: { relays: string[]; event: NostrEvent }[] = [];
      const signer = makeSigner(
        { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
        published,
        { confirmGate: () => true },
      );
      await signer.connect("local");

      await signer.signDecision("req_1", "approved", appOpenCtx());

      expect(published).toHaveLength(1);
      const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
      expect(got.valid).toBe(true);
      expect(got.payload).toMatchObject({
        v: 1,
        op: "app.open",
        reqId: REQ_ID,
        nonce: NONCE,
        decision: "allow",
        params: { pkg: PKG, minutesGranted: 30 },
      });
      expect(got.payload.exp).toBeGreaterThan(got.payload.ts);
      // Only the asking device can open it.
      expect(() => unwrapGrant(published[0].event, DEV2, GUARDIAN)).toThrow();
    });

    it("deny publishes a deny GRANT carrying 0 minutes — no clause change at all", async () => {
      const published: { relays: string[]; event: NostrEvent }[] = [];
      const signer = makeSigner(
        { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
        published,
        { confirmGate: () => true },
      );
      await signer.connect("local");

      await signer.signDecision("req_1", "denied", appOpenCtx({ minutesGranted: 0 }));

      const got = unwrapGrant(published[0].event, DEV1, GUARDIAN);
      expect(got.payload.decision).toBe("deny");
      expect(got.payload.params).toEqual({ pkg: PKG, minutesGranted: 0 });
    });

    it("never needs a clause tz — app.open has no end-of-day to compute", async () => {
      const published: { relays: string[]; event: NostrEvent }[] = [];
      const signer = makeSigner(
        { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
        published,
        { confirmGate: () => true },
      );
      await signer.connect("local");

      await expect(
        signer.signDecision("req_1", "approved", appOpenCtx({ tz: undefined })),
      ).resolves.toEqual({ ok: true });
      expect(published).toHaveLength(1);
    });
  });
});

describe("RealSigner.signGiftClause", () => {
  it("publishes a today-only gift to every device, with a unique id per gift", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signGiftClause!("child_sam", 30, "Europe/London");
    expect(published).toHaveLength(2); // one per device

    const unwrapped = published
      .map((p) => {
        try {
          return unwrapClause(p.event, DEV1, GUARDIAN);
        } catch {
          return null;
        }
      })
      .filter((u): u is NonNullable<typeof u> => u !== null);
    expect(unwrapped).toHaveLength(1);
    const body = unwrapped[0].payload.body as {
      minutes: number;
      id: string;
      expiresAt: number;
      issuedAt: number;
    };
    expect(unwrapped[0].valid).toBe(true);
    expect(unwrapped[0].payload.kind).toBe("gift");
    expect(unwrapped[0].payload.subject).toBe(SUBJECT);
    expect(body.minutes).toBe(30);
    // Dies at end of the CHILD's day, and is always strictly in the future.
    expect(body.expiresAt).toBeGreaterThan(body.issuedAt);

    // A second gift must be a NEW id — that is the whole reason a re-read
    // clause applies once but two gifts add twice.
    published.length = 0;
    await signer.signGiftClause!("child_sam", 15, "Europe/London");
    const second = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect((second.payload.body as { id: string }).id).not.toBe(body.id);
  });

  // Named times: a gift can top up ONE group's own pool instead of the
  // whole-device one — additive (v: 1 unchanged), so an old ward simply
  // ignores the key and lands the gift as device time anyway.
  it("carries groupId when the guardian picked a group, and omits it for the whole day", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signGiftClause!("child_sam", 15, "Europe/London", "play");
    const withGroup = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect((withGroup.payload.body as { groupId?: string }).groupId).toBe("play");

    published.length = 0;
    await signer.signGiftClause!("child_sam", 15, "Europe/London");
    const wholeDay = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect("groupId" in (wholeDay.payload.body as object)).toBe(false);
  });

  it("clamps an absurd gift rather than putting it on the wire", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signGiftClause!("child_sam", 99_999, "Europe/London");
    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect((u.payload.body as { minutes: number }).minutes).toBeLessThanOrEqual(1440);
  });

  // A gift is worth only what arrives: "extra time for today only". The publish
  // loop is `for (const device of target.devices)` over an unconditional
  // `return { ok: true }`, so an empty list used to resolve happily having sent
  // nothing — and GiveTime prints "N minutes sent to <ward>" on a true return.
  // The guardian is then certain they unlocked their ward while the ward stays
  // locked, and neither end shows a thing. Refuse instead, out loud.
  it("refuses a gift it cannot deliver rather than reporting minutes sent", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: SUBJECT, devices: [], relays: RELAYS }, published);
    await signer.connect("signet");

    await expect(
      signer.signGiftClause!("child_sam", 15, "Europe/London"),
    ).rejects.toThrow(/no paired device/i);
    expect(published).toHaveLength(0);
  });

  it("refuses a gift when there is no relay to carry it", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: [] },
      published,
    );
    await signer.connect("signet");

    await expect(
      signer.signGiftClause!("child_sam", 15, "Europe/London"),
    ).rejects.toThrow(/no relay/i);
    expect(published).toHaveLength(0);
  });

  // The counterpart, and the reason the guard is not blanket: a charter is
  // durable state that legitimately predates any phone. Setting rules for a
  // ward whose phone is not paired yet must still SAVE — `signClauseIfConnected`
  // applies the change locally only once signing resolves, so a refusal here
  // would stop a guardian writing rules at all before pairing day.
  it("still signs a rule change for a ward with no paired phone yet", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: SUBJECT, devices: [], relays: RELAYS }, published);
    await signer.connect("signet");

    await expect(signer.signClause("child_sam", devicePolicy)).resolves.toEqual({ ok: true });
    expect(published).toHaveLength(0);
  });
});

describe("RealSigner.signStandDownClause", () => {
  it("publishes a stand-down to every device, with a fresh id and a midnight lapse", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signStandDownClause!("child_sam", { tz: "Europe/London" });
    expect(published).toHaveLength(2); // one per device

    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.valid).toBe(true);
    expect(u.payload.kind).toBe("standdown");
    const body = u.payload.body as {
      id: string;
      expiresAt: number;
      issuedAt: number;
      graceSecs: number;
    };
    expect(body.graceSecs).toBe(60);
    // Lapses at end of the WARD's day, strictly in the future.
    expect(body.expiresAt).toBeGreaterThan(body.issuedAt);

    // A second stand-down is a NEW id, so the device pins its grace afresh
    // rather than treating it as the one it already started counting.
    published.length = 0;
    await signer.signStandDownClause!("child_sam", { tz: "Europe/London" });
    const second = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect((second.payload.body as { id: string }).id).not.toBe(body.id);
  });

  /// Lifting is an ALREADY-EXPIRED stand-down: one code path both calls and
  /// clears, and the device's "highest issuedAt wins, expired doesn't stand"
  /// rule means a lift can never be reordered behind the call it lifts.
  it("lifts by publishing a stand-down that has already expired", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signStandDownClause!("child_sam", { lift: true });
    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    const body = u.payload.body as { expiresAt: number; issuedAt: number };
    expect(u.payload.kind).toBe("standdown");
    expect(body.expiresAt).toBeLessThanOrEqual(body.issuedAt);
  });

  // Same refusal as a gift: its whole value is arriving now, so claiming it
  // travelled when there was no recipient is the lie worth preventing.
  it("refuses a stand-down it cannot deliver", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner({ subject: SUBJECT, devices: [], relays: RELAYS }, published);
    await signer.connect("signet");

    await expect(signer.signStandDownClause!("child_sam")).rejects.toThrow(/no paired device/i);
    expect(published).toHaveLength(0);
  });

  // A guardian reads these errors verbatim; `child_lx3k9_4` is ours, not hers.
  it("names the child, not the internal id, when there is nothing to deliver to", async () => {
    const signer = makeSigner(
      { subject: SUBJECT, name: "Sam", devices: [], relays: RELAYS },
      [],
    );
    await signer.connect("signet");

    const err = await signer.signStandDownClause!("child_sam").catch((e: Error) => e);
    expect(err).toBeInstanceOf(Error);
    expect((err as Error).message).toContain("Sam");
    expect((err as Error).message).not.toContain("child_sam");
  });

  // A lift must free the ward even on a device whose clock runs BEHIND the
  // guardian's. Stamped `expiresAt: issuedAt`, a slow-clocked warden saw the
  // expiry in its own future and read the lift as a fresh stand-down — a
  // spurious warning, and past 60s of skew, re-locked by the very message
  // meant to free her. The device now also recognises `expiresAt <= issuedAt`
  // clock-independently; stamping it firmly in the past covers the wardens
  // shipped before that rule.
  it("stamps a lift's expiry firmly in the past, beyond any plausible skew", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signStandDownClause!("child_sam", { lift: true });
    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    const body = u.payload.body as { expiresAt: number; issuedAt: number };
    expect(body.issuedAt - body.expiresAt).toBeGreaterThanOrEqual(86_400);
  });

  // The device's per-kind monotonic floor drops a clause whose issuedAt does
  // not EXCEED the standing one's, and issuedAt is whole seconds — so calling
  // and lifting within one second silently dropped the lift: the app said
  // "allowed back on" while the ward stayed locked until midnight.
  it("a lift signed in the same second as the call still supersedes it", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signStandDownClause!("child_sam", { tz: "Europe/London" });
    const call = unwrapClause(published[0].event, DEV1, GUARDIAN).payload.body as {
      issuedAt: number;
    };
    published.length = 0;
    await signer.signStandDownClause!("child_sam", { lift: true });
    const lift = unwrapClause(published[0].event, DEV1, GUARDIAN).payload.body as {
      issuedAt: number;
    };
    expect(lift.issuedAt).toBeGreaterThan(call.issuedAt);
  });
});

// The same floor applies to EVERY kind, not just stand-down: closing a
// maintenance window opened in the same second used to be dropped, leaving
// the window open while the app said it was shut.
describe("RealSigner clause clock", () => {
  it("never signs two clauses of a kind at one issuedAt, across a reload too", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const target = { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS };
    const signer = makeSigner(target, published);
    await signer.connect("signet");

    await signer.signMaintenanceClause!("child_sam", 30);
    await signer.signMaintenanceClause!("child_sam", 0);
    // A fresh signer (the app reloaded) inside the same frozen second.
    const reloaded = makeSigner(target, published);
    await reloaded.connect("signet");
    await reloaded.signMaintenanceClause!("child_sam", 15);

    const stamps = published.map(
      (p) => (unwrapClause(p.event, DEV1, GUARDIAN).payload as { issuedAt: number }).issuedAt,
    );
    expect(stamps).toHaveLength(3);
    expect(stamps[1]).toBeGreaterThan(stamps[0]);
    expect(stamps[2]).toBeGreaterThan(stamps[1]);
  });
});

describe("RealSigner.signMaintenanceClause", () => {
  it("opens a window with an ABSOLUTE expiry, to every device", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signMaintenanceClause!("child_sam", 30);
    expect(published).toHaveLength(2); // one per device

    const u = unwrapClause(published[0].event, DEV1, GUARDIAN);
    expect(u.valid).toBe(true);
    expect(u.payload.kind).toBe("maintenance");
    const body = u.payload.body as { untilUnix: number; issuedAt: number };
    // A DURATION would restart on every re-read of the stored clause and the
    // window would never shut. It is a timestamp, and it is 30 minutes out.
    expect(body.untilUnix).toBe(body.issuedAt + 30 * 60);
  });

  /**
   * Closing early is an ALREADY-SHUT window, exactly as lifting a stand-down is
   * an already-expired one: the ward's `is_open` shuts on `untilUnix <= now`,
   * and the store's monotonic `issuedAt` floor makes the newer clause supersede
   * the open one. The point is that it needs NO new ward code — a guardian
   * ending a loosening early must never wait on a phone release.
   */
  it("closes a window early by publishing one that has already shut", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(
      { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }], relays: RELAYS },
      published,
    );
    await signer.connect("signet");

    await signer.signMaintenanceClause!("child_sam", 30);
    const open = unwrapClause(published[0].event, DEV1, GUARDIAN).payload.body as {
      untilUnix: number;
      issuedAt: number;
    };

    published.length = 0;
    await signer.signMaintenanceClause!("child_sam", 0);
    const shut = unwrapClause(published[0].event, DEV1, GUARDIAN).payload.body as {
      untilUnix: number;
      issuedAt: number;
    };

    // Already shut at the instant it was issued...
    expect(shut.untilUnix).toBeLessThanOrEqual(shut.issuedAt);
    // ...and never older than the window it is closing, or the store's floor
    // would drop it and the window would stay open to its full length.
    expect(shut.issuedAt).toBeGreaterThanOrEqual(open.issuedAt);
  });
});

describe("RealSigner keeps a rule change's signed clauses for the family roster (Q3)", () => {
  it("hands on one family-wide signed inner per kind once sent, and nothing when every send failed", async () => {
    const kept: { childId: string; signed: SignedRule[] }[] = [];
    const target = { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS };
    const signer = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async () => {},
      resolveChild: () => target,
      now: () => 1_700_000_000,
      onRulesSigned: (childId, signed) => kept.push({ childId, signed }),
    });
    await signer.connect("local");
    await signer.signClause("child_sam", devicePolicy);
    expect(kept).toHaveLength(1);
    expect(kept[0].childId).toBe("child_sam");
    expect(kept[0].signed.map((r) => JSON.parse(r.inner.content).kind).sort()).toEqual(["budget", "schedule"]);
    for (const r of kept[0].signed) {
      expect(r.machine).toBeUndefined();
      expect(r.inner.kind).toBe(31113);
      expect(r.inner.pubkey).toBe(GUARDIAN);
    }

    const failing = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async () => {
        throw new Error("no relay");
      },
      resolveChild: () => target,
      now: () => 1_700_000_000,
      onRulesSigned: (childId, signed) => kept.push({ childId, signed }),
    });
    await failing.connect("local");
    await expect(failing.signClause("child_sam", devicePolicy)).rejects.toThrow();
    expect(kept).toHaveLength(1);
  });

  it("keeps a rule per device when the devices' clauses differ, and the device kinds always per device", async () => {
    const kept: SignedRule[] = [];
    const target = { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS };
    const signer = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async () => {},
      resolveChild: () => target,
      now: () => 1_700_000_000,
      onRulesSigned: (_c, signed) => kept.push(...signed),
    });
    await signer.connect("local");
    const split: Policy = {
      ...devicePolicy,
      deviceOverrides: { dev2: { budget: { ...devicePolicy.budget!, dailyMinutes: 30 } } },
    };
    await signer.signClause("child_sam", split);
    const view = kept.map((r) => [JSON.parse(r.inner.content).kind, r.machine ?? null]).sort();
    expect(view).toEqual(
      [
        ["budget", DPK1],
        ["budget", DPK2],
        ["schedule", null],
      ].sort(),
    );

    kept.length = 0;
    await signer.signDebuggingClause("child_sam", DPK2, { allow: true });
    expect(kept.map((r) => [JSON.parse(r.inner.content).kind, r.machine])).toEqual([["debugging", DPK2]]);
    kept.length = 0;
    await signer.signMaintenanceClause("child_sam", 10);
    expect(kept.map((r) => r.machine).sort()).toEqual([DPK1, DPK2].sort());
  });
});

describe("RealSigner sends a newly claimed device the rules in force: its own as they are, the other parent's re-signed (S1, SF3)", () => {
  const T0 = 1_699_000_000;
  const heldClause = (sk: Uint8Array, kind: string, issuedAt: number) =>
    finalizeEvent(
      {
        kind: 31113,
        created_at: issuedAt,
        tags: [["t", "charter-device"]],
        content: JSON.stringify({ v: 1, kind, subject: SUBJECT, issuedAt, body: { v: 1, tz: "UTC", revoked: true, issuedAt } }),
      },
      sk,
    );
  const target = { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK1 }, { id: "dev2", pubkey: DPK2 }], relays: RELAYS };
  const setup = (held: { kind: string; event: NostrEvent; local: boolean }[]) => {
    const published: NostrEvent[] = [];
    const kept: SignedRule[] = [];
    const asked: string[] = [];
    const signer = new RealSigner({
      connectGuardian: async () => localGuardian(),
      publish: async (_r, wrap) => {
        published.push(wrap);
      },
      resolveChild: () => target,
      now: () => 1_700_000_000,
      onRulesSigned: (_c, signed) => kept.push(...signed),
      heldRules: (_c, machine) => {
        asked.push(machine);
        return held;
      },
    });
    return { signer, published, kept, asked };
  };

  it("re-signs its own held rules with their original issuedAt, to the claimed device only, without moving the clock", async () => {
    // This install once ran ahead: its clock is past the held rule's date.
    localStorage.setItem("charter.signer.lastIssuedAt", "1700000500");
    const mine = heldClause(GSK, "schedule", T0);
    const myBudget = heldClause(GSK, "budget", T0 + 5);
    const { signer, published, kept, asked } = setup([
      { kind: "schedule", event: mine, local: true },
      { kind: "budget", event: myBudget, local: true },
    ]);
    await signer.connect("local");
    await signer.resendRulesTo("child_sam", DPK2, [devicePolicy]);
    expect(asked).toEqual([DPK2]);
    // Device A gets nothing; B gets this install's two rules.
    expect(published).toHaveLength(2);
    for (const w of published) expect(w.tags.filter((t) => t[0] === "p").map((t) => t[1])).toEqual([DPK2]);
    const { payload, inner, valid } = unwrapClause(published[0], DEV2, GUARDIAN);
    expect(valid).toBe(true);
    expect(payload).toEqual(JSON.parse(mine.content));
    expect(payload.issuedAt).toBe(T0);
    expect(inner.created_at).toBe(T0);
    expect(published[0].created_at).toBeGreaterThan(T0);
    expect(unwrapClause(published[1], DEV2, GUARDIAN).payload).toEqual(JSON.parse(myBudget.content));
    expect(localStorage.getItem("charter.signer.lastIssuedAt")).toBe("1700000500");
    expect(kept).toEqual([]);
  });

  it("re-signs the other parent's rules in force too, same body, to the claimed device only, without moving the clock or keeping them as its own (SF3)", async () => {
    localStorage.setItem("charter.signer.lastIssuedAt", "1700000500");
    const otherSk = generateSecretKey();
    const mine = heldClause(GSK, "schedule", T0);
    const theirs = heldClause(otherSk, "budget", T0 + 10);
    const { signer, published, kept } = setup([
      { kind: "schedule", event: mine, local: true },
      { kind: "budget", event: theirs, local: false },
    ]);
    await signer.connect("local");
    await signer.resendRulesTo("child_sam", DPK2, [devicePolicy]);
    // Both parents' rules, once per kind, to B alone.
    expect(published).toHaveLength(2);
    for (const w of published) expect(w.tags.filter((t) => t[0] === "p").map((t) => t[1])).toEqual([DPK2]);
    const got = published.map((w) => unwrapClause(w, DEV2, GUARDIAN));
    for (const g of got) {
      expect(g.valid).toBe(true);
      expect(g.inner.pubkey).toBe(getPublicKey(GSK)); // the claimer's key: the device pins only its root
    }
    const byKind = Object.fromEntries(got.map((g) => [g.payload.kind, g]));
    expect(byKind.schedule.payload).toEqual(JSON.parse(mine.content));
    // The other parent's: their body, their issuedAt (already in the past).
    expect(byKind.budget.payload).toEqual(JSON.parse(theirs.content));
    expect(byKind.budget.inner.created_at).toBe(T0 + 10);
    // Not new rules: the clock is where it was, and nothing is kept as this install's.
    expect(localStorage.getItem("charter.signer.lastIssuedAt")).toBe("1700000500");
    expect(kept).toEqual([]);
  });

  it("re-signs a departed signer's rule in force too, since the child's other devices still enforce it (rule 4)", async () => {
    localStorage.setItem("charter.signer.lastIssuedAt", "1700000500");
    // A rule from a signer that has since departed (no ward reports it).
    const departed = heldClause(generateSecretKey(), "budget", T0 + 10);
    const { signer, published, kept } = setup([{ kind: "budget", event: departed, local: false }]);
    await signer.connect("local");
    await signer.resendRulesTo("child_sam", DPK2, [devicePolicy]);
    const got = published.map((w) => unwrapClause(w, DEV2, GUARDIAN));
    const budget = got.find((g) => g.payload.kind === "budget")!;
    expect(budget.valid).toBe(true);
    expect(budget.inner.pubkey).toBe(getPublicKey(GSK));
    expect(budget.payload).toEqual(JSON.parse(departed.content));
    expect(budget.inner.created_at).toBe(T0 + 10);
    for (const w of published) expect(w.tags.filter((t) => t[0] === "p").map((t) => t[1])).toEqual([DPK2]);
    // Not kept as this install's rule (only the schedule, sent fresh, is).
    expect(kept.map((r) => JSON.parse(r.inner.content).kind)).toEqual(["schedule"]);
  });

  it("dates the other parent's far-future rule at now, so it never sits on the claimer's floor above its next rule (SF3)", async () => {
    const ahead = 1_700_000_000 + 86_400;
    const theirs = heldClause(generateSecretKey(), "budget", ahead);
    const { signer, published } = setup([{ kind: "budget", event: theirs, local: false }]);
    await signer.connect("local");
    await signer.resendRulesTo("child_sam", DPK2, [devicePolicy]);
    const budget = published.map((w) => unwrapClause(w, DEV2, GUARDIAN)).find((g) => g.payload.kind === "budget")!;
    expect(budget.payload.issuedAt).toBe(1_700_000_000);
    expect(budget.inner.created_at).toBe(1_700_000_000);
    // The body is theirs, unaltered.
    expect(budget.payload.body).toEqual(JSON.parse(theirs.content).body);
  });

  it("dates every clause wrap with the raw clock, so a rising clock days ahead after a correction still reaches the ward (SF1)", async () => {
    // Once three days ahead, now corrected: the rising clock is still there.
    localStorage.setItem("charter.signer.lastIssuedAt", String(1_700_000_000 + 3 * 86_400));
    const { signer, published } = setup([]);
    await signer.connect("local");
    await signer.signClause("child_sam", devicePolicy);
    expect(published.length).toBeGreaterThan(0);
    for (const w of published) {
      // The ward refuses a wrap more than two days from its clock (NIP-59 jitter).
      expect(w.created_at).toBe(1_700_000_000);
      const opened = unwrapClause(w, w.tags.find((t) => t[0] === "p")![1] === DPK1 ? DEV1 : DEV2, GUARDIAN);
      expect(opened.payload.issuedAt).toBe(1_700_000_000 + 3 * 86_400 + 1);
    }
  });

  it("counts a gift's, a stand-down's and an install window's expiry from the raw clock, not a rising clock left ahead", async () => {
    // Once three days ahead, now corrected: the rising clock is still there.
    const now = 1_700_000_000;
    const ahead = now + 3 * 86_400;
    localStorage.setItem("charter.signer.lastIssuedAt", String(ahead));
    const { signer, published } = setup([]);
    await signer.connect("local");
    const sent = async (sign: () => Promise<unknown>) => {
      published.length = 0;
      await sign();
      const opened = published.map((w) => unwrapClause(w, w.tags.find((t) => t[0] === "p")![1] === DPK1 ? DEV1 : DEV2, GUARDIAN));
      expect(opened.length).toBeGreaterThan(0);
      for (const o of opened) expect(o.payload.issuedAt).toBeGreaterThan(ahead);
      return opened[0].payload.body as { expiresAt?: number; untilUnix?: number; issuedAt: number };
    };
    // A gift dies at the end of the child's day as the raw clock reads it.
    const gift = await sent(() => signer.signGiftClause!("child_sam", 30, "Europe/London"));
    expect(gift.expiresAt).toBe(endOfDayUnix(now, "Europe/London"));
    // A stand-down likewise; a lift is a day before the raw clock, so before
    // both the ward's clock and its own issuedAt.
    const call = await sent(() => signer.signStandDownClause!("child_sam", { tz: "Europe/London" }));
    expect(call.expiresAt).toBe(endOfDayUnix(now, "Europe/London"));
    const lift = await sent(() => signer.signStandDownClause!("child_sam", { lift: true, tz: "Europe/London" }));
    expect(lift.expiresAt).toBe(now - 86_400);
    // A 30-minute install window is 30 minutes of real time.
    const window = await sent(() => signer.signMaintenanceClause("child_sam", 30));
    expect(window.untilUnix).toBe(now + 30 * 60);
  });

  it("sends a kind with no rule held for the device as a fresh rule, to that device alone", async () => {
    localStorage.setItem("charter.signer.lastIssuedAt", "1700000500");
    const theirs = heldClause(generateSecretKey(), "budget", T0 + 10);
    const { signer, published, kept } = setup([{ kind: "budget", event: theirs, local: false }]);
    await signer.connect("local");
    await signer.resendRulesTo("child_sam", DPK2, [devicePolicy]);
    const kinds = published.map((w) => unwrapClause(w, DEV2, GUARDIAN).payload);
    // Their budget as it is held; the schedule, held for no one, fresh.
    expect(kinds.map((p) => p.kind)).toEqual(["budget", "schedule"]);
    expect(kinds[0]).toEqual(JSON.parse(theirs.content));
    expect(kinds[1].issuedAt).toBe(1_700_000_501);
    for (const w of published) expect(w.tags.filter((t) => t[0] === "p").map((t) => t[1])).toEqual([DPK2]);
    // Kept for the roster as this device's rule, not a family-wide one.
    expect(kept.map((r) => [JSON.parse(r.inner.content).kind, r.machine])).toEqual([["schedule", DPK2]]);
  });
});
