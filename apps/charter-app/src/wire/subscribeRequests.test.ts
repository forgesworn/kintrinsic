import { describe, it, expect } from "vitest";
import {
  generateSecretKey,
  getPublicKey,
  finalizeEvent,
  nip44,
  type NostrEvent,
} from "nostr-tools";
import { subscribeRequests, type RequestSubPool } from "./subscribeRequests";
import { CHARTER_DEVICE_REQUEST, type DeviceRequest, type TimeExtendDeviceRequest } from "./request";

const SEAL = 13;
const GIFT_WRAP = 1059;
const MARKER: [string, string] = ["t", "charter-device"];

function sample(
  machine: string,
  over: Partial<TimeExtendDeviceRequest> = {},
): DeviceRequest {
  return {
    v: 1,
    op: "time.extend",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine,
    ts: 1_700_000_000,
    params: { minutesRequested: 30, reason: "homework's done", limitHit: "budget" },
    ...over,
  };
}

// Mirror the device's submit_request wrap EXACTLY: the REQUEST rumor is
// UNSIGNED (charterd publishes it bare — no id, no sig); the machine's schnorr
// signature lives on the SEAL; the outer 1059 is ephemeral-authored.
function giftWrapRequest(
  request: DeviceRequest,
  machineSk: Uint8Array,
  guardianPk: string,
  overrides: {
    rumorTags?: string[][];
    rumorPubkey?: string;
    rumorKind?: number;
    wrapCreatedAt?: number;
  } = {},
): NostrEvent {
  const rumor = {
    kind: overrides.rumorKind ?? CHARTER_DEVICE_REQUEST,
    pubkey: overrides.rumorPubkey ?? getPublicKey(machineSk),
    created_at: request.ts,
    tags: overrides.rumorTags ?? [MARKER],
    content: JSON.stringify(request),
  };
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: request.ts,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, guardianPk)),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: overrides.wrapCreatedAt ?? request.ts,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

/** A fake pool that captures the onevent handler so the test can feed it events. */
function fakePool() {
  let onevent: ((e: NostrEvent) => void) | undefined;
  let filter: unknown;
  let relays: string[] | undefined;
  let closed = false;
  const pool: RequestSubPool = {
    subscribeMany(r, f, params) {
      relays = r;
      filter = f;
      onevent = params.onevent;
      return {
        close() {
          closed = true;
        },
      };
    },
  };
  return {
    pool,
    fire: (e: NostrEvent) => onevent!(e),
    getFilter: () => filter,
    getRelays: () => relays,
    isClosed: () => closed,
  };
}

function subscribe(fp: ReturnType<typeof fakePool>, guardianSk: Uint8Array, seen: DeviceRequest[]) {
  return subscribeRequests(fp.pool, {
    relays: ["wss://relay.example"],
    guardianPubkey: getPublicKey(guardianSk),
    guardianSk,
    onRequest: (r) => seen.push(r),
    // Pin the clock to the sample wraps' era so the freshness bound is exercised
    // deterministically (the stale-wrap test below moves the wrap, not the clock).
    now: () => 1_700_000_000,
  });
}

describe("subscribeRequests", () => {
  it("subscribes for 1059 wraps p-tagged to the guardian", () => {
    const guardianSk = generateSecretKey();
    const fp = fakePool();
    subscribe(fp, guardianSk, []);
    expect(fp.getRelays()).toEqual(["wss://relay.example"]);
    expect(fp.getFilter()).toEqual({ kinds: [GIFT_WRAP], "#p": [getPublicKey(guardianSk)] });
  });

  it("delivers a parsed request end-to-end (bare rumor, machine-signed seal)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const seen: DeviceRequest[] = [];
    const fp = fakePool();
    const sub = subscribe(fp, guardianSk, seen);

    const r = sample(getPublicKey(machineSk));
    fp.fire(giftWrapRequest(r, machineSk, getPublicKey(guardianSk)));
    expect(seen).toEqual([r]);

    // A wrap for a DIFFERENT recipient / garbage decrypts to nothing → dropped.
    fp.fire(giftWrapRequest(r, machineSk, getPublicKey(generateSecretKey())));
    expect(seen).toHaveLength(1);

    sub.close();
    expect(fp.isClosed()).toBe(true);
  });

  it("dedupes a re-delivered reqId (relay replay fires onRequest once)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const seen: DeviceRequest[] = [];
    const fp = fakePool();
    subscribe(fp, guardianSk, seen);

    const r = sample(getPublicKey(machineSk));
    const wrap = giftWrapRequest(r, machineSk, guardianPk);
    fp.fire(wrap);
    fp.fire(wrap); // the same stored wrap replayed on reconnect
    fp.fire(giftWrapRequest(r, machineSk, guardianPk)); // same reqId, fresh wrap
    expect(seen).toHaveLength(1);

    // A DIFFERENT reqId still gets through.
    fp.fire(giftWrapRequest(sample(getPublicKey(machineSk), { reqId: "12".repeat(32) }), machineSk, guardianPk));
    expect(seen).toHaveLength(2);
  });

  it("drops out-of-range minutes, a missing marker tag, and a wrong inner kind", () => {
    const machineSk = generateSecretKey();
    const machinePk = getPublicKey(machineSk);
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const seen: DeviceRequest[] = [];
    const fp = fakePool();
    subscribe(fp, guardianSk, seen);

    const zero = sample(machinePk, { params: { minutesRequested: 0, limitHit: "budget" } });
    fp.fire(giftWrapRequest(zero, machineSk, guardianPk));
    const over = sample(machinePk, { params: { minutesRequested: 1441, limitHit: "budget" } });
    fp.fire(giftWrapRequest(over, machineSk, guardianPk));
    fp.fire(giftWrapRequest(sample(machinePk), machineSk, guardianPk, { rumorTags: [] }));
    fp.fire(giftWrapRequest(sample(machinePk), machineSk, guardianPk, { rumorKind: 31114 }));
    expect(seen).toHaveLength(0);
  });

  it("drops a wrap outside the freshness window (stale replay / future-dated)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const seen: DeviceRequest[] = [];
    const fp = fakePool();
    subscribe(fp, guardianSk, seen);

    const twoDays = 2 * 24 * 60 * 60;
    const r = sample(getPublicKey(machineSk));
    fp.fire(
      giftWrapRequest(r, machineSk, guardianPk, {
        wrapCreatedAt: 1_700_000_000 - twoDays - 1,
      }),
    );
    fp.fire(
      giftWrapRequest(r, machineSk, guardianPk, {
        wrapCreatedAt: 1_700_000_000 + twoDays + 1,
      }),
    );
    expect(seen).toHaveLength(0);
  });

  it("drops a request whose payload.machine is not the sealed author (misattribution)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const seen: DeviceRequest[] = [];
    const fp = fakePool();
    subscribe(fp, guardianSk, seen);

    // The payload claims ANOTHER device; the rumor/seal author is machineSk.
    const forged = sample(getPublicKey(generateSecretKey()));
    fp.fire(giftWrapRequest(forged, machineSk, getPublicKey(guardianSk)));
    // A rumor claiming another author than the seal's is equally dropped.
    const honest = sample(getPublicKey(machineSk));
    fp.fire(
      giftWrapRequest(honest, machineSk, getPublicKey(guardianSk), {
        rumorPubkey: getPublicKey(generateSecretKey()),
      }),
    );
    expect(seen).toHaveLength(0);
  });
});
