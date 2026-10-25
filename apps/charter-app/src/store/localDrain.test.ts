import { describe, expect, it, vi } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type NostrEvent,
} from "nostr-tools";
import { CHARTER_DEVICE_REQUEST, type DeviceRequest } from "../wire/request";
import type { LocalInboxEntry } from "../carrier/localLink";
import { ingestWrap, type WrapIngestContext } from "./ingestWrap";
import { ConsumeQueue, drainLocalInbox } from "./localDrain";

const MARKER: [string, string] = ["t", "charter-device"];

const guardianSk = generateSecretKey();
const guardianPk = getPublicKey(guardianSk);
const machineSk = generateSecretKey();
const machinePk = getPublicKey(machineSk);

function request(reqId: string, over: Partial<DeviceRequest> = {}): DeviceRequest {
  return {
    v: 1,
    op: "time.extend",
    reqId,
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: machinePk,
    ts: Math.floor(Date.now() / 1000),
    params: { minutesRequested: 30, reason: "homework", limitHit: "budget" },
    ...over,
  } as DeviceRequest;
}

// Mirrors the device's submit_request wrap: unsigned rumor, machine-signed seal,
// ephemeral-authored outer 1059.
function giftWrapRequest(req: DeviceRequest, opts: { sealedBy?: Uint8Array } = {}): NostrEvent {
  const sk = opts.sealedBy ?? machineSk;
  const rumor = {
    kind: CHARTER_DEVICE_REQUEST,
    pubkey: getPublicKey(machineSk),
    created_at: req.ts,
    tags: [MARKER],
    content: JSON.stringify(req),
  };
  const seal = finalizeEvent(
    {
      kind: 13,
      created_at: req.ts,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(sk, guardianPk)),
    },
    sk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: 1059,
      created_at: req.ts,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

/** A miniature of the provider's state: in-memory vs persisted requests. */
function makeStore(opts: { knownMachine?: boolean } = {}) {
  const known = opts.knownMachine ?? true;
  const memory: string[] = [];
  const persisted: string[] = [];
  const statuses: unknown[] = [];
  const ctx: WrapIngestContext = {
    guardianSk,
    handledWraps: new Set(),
    seenReqIds: new Set(),
    hasRequest: (reqId) => memory.includes(reqId),
    ingestStatus: (s) => statuses.push(s),
    ingestRequest: (r) => {
      if (!known) return false;
      if (!memory.includes(r.reqId)) memory.push(r.reqId);
      return true;
    },
  };
  return {
    ctx,
    memory,
    persisted,
    statuses,
    persist: () => {
      persisted.splice(0, persisted.length, ...memory);
    },
  };
}

const entry = (w: NostrEvent): LocalInboxEntry => ({ id: w.id, machine: machinePk, wrap: w, receivedAt: 1 });

describe("ingestWrap (the intake both routes share)", () => {
  it("lands a request once and calls the second copy a duplicate", () => {
    const store = makeStore();
    const w = giftWrapRequest(request("aa".repeat(32)));
    expect(ingestWrap(w, store.ctx)).toEqual({ kind: "request", reqId: "aa".repeat(32) });
    expect(ingestWrap(w, store.ctx)).toEqual({ kind: "handled" });
    // A different wrap (new ephemeral key) carrying the same ask: dedupe by reqId.
    const again = giftWrapRequest(request("aa".repeat(32)));
    expect(ingestWrap(again, store.ctx)).toEqual({ kind: "duplicate", reqId: "aa".repeat(32) });
    expect(store.memory).toEqual(["aa".repeat(32)]);
  });

  it("holds back an ask from an unknown phone, unconsumed, so a later pass can land it", () => {
    const store = makeStore({ knownMachine: false });
    const w = giftWrapRequest(request("bb".repeat(32)));
    expect(ingestWrap(w, store.ctx)).toEqual({ kind: "unlanded", reqId: "bb".repeat(32) });
    expect(store.ctx.handledWraps.has(w.id)).toBe(false);
  });

  it("rejects a wrap sealed by a key other than the machine it names", () => {
    const store = makeStore();
    const forged = giftWrapRequest(request("cc".repeat(32)), { sealedBy: generateSecretKey() });
    expect(ingestWrap(forged, store.ctx).kind).toBe("unreadable");
    expect(store.memory).toEqual([]);
  });
});

describe("home-network drain", () => {
  it("feeds each inbox wrap through the same intake and consumes only after persistence", () => {
    const store = makeStore();
    const w = giftWrapRequest(request("dd".repeat(32)));
    const queue = new ConsumeQueue();
    const consume = vi.fn();

    drainLocalInbox({ inbox: () => [entry(w)], ingest: (x) => ingestWrap(x, store.ctx), queue });
    expect(store.memory).toEqual(["dd".repeat(32)]);

    // State not written yet: the carrier must keep the wrap.
    queue.flush({ persistedOk: true, hasRequest: (id) => store.persisted.includes(id), consume });
    expect(consume).not.toHaveBeenCalled();

    // A failed write must not release it either.
    store.persist();
    queue.flush({ persistedOk: false, hasRequest: (id) => store.persisted.includes(id), consume });
    expect(consume).not.toHaveBeenCalled();

    queue.flush({ persistedOk: true, hasRequest: (id) => store.persisted.includes(id), consume });
    expect(consume).toHaveBeenCalledTimes(1);
    expect(consume).toHaveBeenCalledWith([w.id]);
    expect(queue.size).toBe(0);
  });

  it("collapses a wrap that arrives by relay AND the home network into one entry and one request", () => {
    const store = makeStore();
    const w = giftWrapRequest(request("ee".repeat(32)));
    // Relay poll got there first…
    expect(ingestWrap(w, store.ctx).kind).toBe("request");
    // …then the carrier hands up the very same wrap.
    const queue = new ConsumeQueue();
    drainLocalInbox({ inbox: () => [entry(w)], ingest: (x) => ingestWrap(x, store.ctx), queue });
    expect(store.memory).toEqual(["ee".repeat(32)]);
    // It is consumed (so it doesn't come back every two seconds) without a second request.
    const consume = vi.fn();
    queue.flush({ persistedOk: true, hasRequest: () => true, consume });
    expect(consume).toHaveBeenCalledWith([w.id]);
  });

  it("works the other way round: local first, the relay copy is then skipped", () => {
    const store = makeStore();
    const w = giftWrapRequest(request("ff".repeat(32)));
    drainLocalInbox({ inbox: () => [entry(w)], ingest: (x) => ingestWrap(x, store.ctx), queue: new ConsumeQueue() });
    expect(ingestWrap(w, store.ctx).kind).toBe("handled");
    expect(store.memory).toHaveLength(1);
  });

  it("consumes wraps that were rejected or are malformed, so they cannot loop", () => {
    const store = makeStore();
    const forged = giftWrapRequest(request("11".repeat(32)), { sealedBy: generateSecretKey() });
    const malformed = { id: "bad", machine: machinePk, wrap: { nonsense: true }, receivedAt: 1 } as unknown as LocalInboxEntry;
    const queue = new ConsumeQueue();
    drainLocalInbox({
      inbox: () => [entry(forged), malformed],
      ingest: (x) => ingestWrap(x, store.ctx),
      queue,
    });
    const consume = vi.fn();
    queue.flush({ persistedOk: true, hasRequest: () => false, consume });
    expect(consume).toHaveBeenCalledWith([forged.id, "bad"]);
    expect(store.memory).toEqual([]);
  });

  it("does not consume an ask it could not land yet", () => {
    const store = makeStore({ knownMachine: false });
    const w = giftWrapRequest(request("22".repeat(32)));
    const queue = new ConsumeQueue();
    drainLocalInbox({ inbox: () => [entry(w)], ingest: (x) => ingestWrap(x, store.ctx), queue });
    expect(queue.size).toBe(0);
  });

  it("a duplicate copy never loosens the wait on a not-yet-persisted request", () => {
    const queue = new ConsumeQueue();
    queue.add("w1", "req1");
    queue.add("w1"); // the same wrap seen again, e.g. by the next drain
    const consume = vi.fn();
    queue.flush({ persistedOk: true, hasRequest: () => false, consume });
    expect(consume).not.toHaveBeenCalled();
    queue.flush({ persistedOk: true, hasRequest: () => true, consume });
    expect(consume).toHaveBeenCalledWith(["w1"]);
  });

  it("does nothing when the bridge has no inbox", () => {
    const ingest = vi.fn();
    expect(drainLocalInbox({ inbox: () => null, ingest, queue: new ConsumeQueue() })).toBe(0);
    expect(ingest).not.toHaveBeenCalled();
  });
});
