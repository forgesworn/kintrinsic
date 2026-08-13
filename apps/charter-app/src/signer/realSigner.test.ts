import { describe, expect, it, vi } from "vitest";
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
import { unwrapClause, unwrapGrant, type GuardianOps } from "../wire/giftwrap";
import type { DecisionContext } from "./Signer";
import { RealSigner, type ChildTarget } from "./realSigner";

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
