import { describe, expect, it } from "vitest";
import { generateSecretKey, getPublicKey, type NostrEvent, type SimplePool } from "nostr-tools";
import type { Policy } from "../domain/types";
import { unwrapClause } from "../wire/giftwrap";
import { createLocalSigner } from "./localSigner";
import type { ChildTarget } from "./realSigner";

const GSK = generateSecretKey();
const GUARDIAN = getPublicKey(GSK);
const DEV = generateSecretKey();
const DPK = getPublicKey(DEV);
const SUBJECT = getPublicKey(generateSecretKey());
const RELAYS = ["wss://relay.trotters.cc"];

const policy: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: { tz: "Europe/London", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
  budget: { tz: "Europe/London", dailyMinutes: 90 },
};

function makeSigner(published: { relays: string[]; event: NostrEvent }[], target: ChildTarget) {
  // Minimal SimplePool stub: capture the publish synchronously, return no promises.
  const pool = {
    publish: (relays: string[], event: NostrEvent) => {
      published.push({ relays, event });
      return [];
    },
  } as unknown as SimplePool;
  return createLocalSigner({ resolveChild: () => target, now: () => 1_700_000_000, loadKey: () => GSK, pool });
}

describe("createLocalSigner relay honesty", () => {
  const target: ChildTarget = {
    subject: SUBJECT,
    devices: [{ id: "dev1", pubkey: DPK }],
    relays: RELAYS,
  };

  // Publishing was fire-and-forget (allSettled — cannot fail): a guardian in
  // airplane mode read "she has a minute, then done for today" while nothing
  // reached any relay. The same "said it travelled when it didn't" class as
  // the no-device and no-signer holes, through the last open door.
  it("refuses when no relay accepted the event", async () => {
    const pool = {
      publish: () => [Promise.reject(new Error("offline"))],
    } as unknown as SimplePool;
    const signer = createLocalSigner({
      resolveChild: () => target,
      now: () => 1_700_000_000,
      loadKey: () => GSK,
      pool,
    });
    await signer.connect("local");

    await expect(signer.signStandDownClause!("child_sam")).rejects.toThrow(/relay/i);
  });

  it("one accepting relay among failures is delivery", async () => {
    const pool = {
      publish: () => [Promise.reject(new Error("offline")), Promise.resolve("ok")],
    } as unknown as SimplePool;
    const signer = createLocalSigner({
      resolveChild: () => target,
      now: () => 1_700_000_000,
      loadKey: () => GSK,
      pool,
    });
    await signer.connect("local");

    await expect(signer.signStandDownClause!("child_sam")).resolves.toEqual({ ok: true });
  });
});

describe("createLocalSigner", () => {
  it("connects offline with the local label", async () => {
    const signer = makeSigner([], { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK }], relays: RELAYS });
    const st = await signer.connect("local");
    expect(st).toMatchObject({ connected: true, kind: "local", label: "This phone" });
  });

  it("signs clauses with the local guardian key and gift-wraps them to the device", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(published, { subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK }], relays: RELAYS });
    await signer.connect("local");

    await signer.signClause("child_sam", policy);

    expect(published).toHaveLength(2); // schedule + budget
    for (const p of published) expect(p.event.kind).toBe(1059);
    const unwrapped = published.map((p) => unwrapClause(p.event, DEV, GUARDIAN));
    expect(unwrapped.every((u) => u.valid && u.payload.subject === SUBJECT)).toBe(true);
    expect(unwrapped.map((u) => u.payload.kind).sort()).toEqual(["budget", "schedule"]);
  });

  it("gates signClause through the confirm step when auto-sign is off, then signs", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const calls: string[] = [];
    const signer = createLocalSigner({
      resolveChild: () => ({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK }], relays: RELAYS }),
      now: () => 1_700_000_000,
      loadKey: () => GSK,
      pool: { publish: (r: string[], e: NostrEvent) => { published.push({ relays: r, event: e }); return []; } } as unknown as SimplePool,
      confirmGate: (ctx) => { calls.push(ctx.action); return true; },
    });
    await signer.connect("local");
    await signer.signClause("child_sam", policy);
    expect(calls).toEqual(["clause"]);      // gate was consulted once
    expect(published).toHaveLength(2);       // and the clauses were signed+published
  });

  it("cancels signClause (publishes nothing) when the confirm step is declined", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = createLocalSigner({
      resolveChild: () => ({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK }], relays: RELAYS }),
      now: () => 1_700_000_000,
      loadKey: () => GSK,
      pool: { publish: (r: string[], e: NostrEvent) => { published.push({ relays: r, event: e }); return []; } } as unknown as SimplePool,
      confirmGate: () => false,
    });
    await signer.connect("local");
    await expect(signer.signClause("child_sam", policy)).rejects.toThrow();
    expect(published).toHaveLength(0);
  });

  // 2026-08-04: a decision on the local signer skips the confirm step
  // entirely — the Approve/Not now press on the request list already IS the
  // confirmation (see mockSigner.ts/realSigner.ts's `authorize`). This used
  // to assert the gate WAS consulted; a clause save still is (see above).
  it("signDecision resolves ok WITHOUT running the confirm step (GRANT delivery still deferred)", async () => {
    const calls: string[] = [];
    const signer = createLocalSigner({
      resolveChild: () => ({ subject: SUBJECT, devices: [{ id: "dev1", pubkey: DPK }], relays: RELAYS }),
      loadKey: () => GSK,
      confirmGate: (ctx) => { calls.push(ctx.action); return true; },
    });
    await signer.connect("local");
    await expect(signer.signDecision("req_1", "approved")).resolves.toEqual({ ok: true });
    expect(calls).toEqual([]);
  });
});
