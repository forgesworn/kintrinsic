import { describe, it, expect } from "vitest";
import {
  generateSecretKey,
  getPublicKey,
  finalizeEvent,
  nip44,
  type NostrEvent,
} from "nostr-tools";
import { subscribeStatus, type StatusSubPool } from "./subscribeStatus";
import { CHARTER_DEVICE_STATUS, type DeviceStatus } from "./status";

const SEAL = 13;
const GIFT_WRAP = 1059;
const MARKER: [string, string] = ["t", "charter-device"];

function sample(machine: string, subject: string): DeviceStatus {
  return {
    v: 1,
    subject,
    machine,
    ts: 1_700_000_000,
    dayKey: "2026-07-02",
    usedTodaySecs: 60,
    windowLeftSecs: 1800,
    quotaLeftSecs: 900,
    effectiveSecs: 900,
    locked: false,
    source: "guardian",
  };
}

// Mirror the device's emit_status wrap (bare machine rumor → seal → gift-wrap).
function giftWrapStatus(
  status: DeviceStatus,
  machineSk: Uint8Array,
  guardianPk: string,
): NostrEvent {
  const rumor = {
    kind: CHARTER_DEVICE_STATUS,
    pubkey: getPublicKey(machineSk),
    created_at: status.ts,
    tags: [MARKER],
    content: JSON.stringify(status),
  };
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: status.ts,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, guardianPk)),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: status.ts,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

/** A fake pool that captures the onevent handler so the test can feed it events. */
function fakePool() {
  let onevent: ((e: NostrEvent) => void) | undefined;
  let filters: unknown;
  let relays: string[] | undefined;
  let closed = false;
  const pool: StatusSubPool = {
    subscribeMany(r, f, params) {
      relays = r;
      filters = f;
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
    getFilters: () => filters,
    getRelays: () => relays,
    isClosed: () => closed,
  };
}

describe("subscribeStatus", () => {
  it("subscribes for 1059 wraps p-tagged to the guardian", () => {
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const fp = fakePool();
    subscribeStatus(fp.pool, {
      relays: ["wss://relay.example"],
      guardianPubkey: guardianPk,
      guardianSk,
      onStatus: () => {},
    });
    expect(fp.getRelays()).toEqual(["wss://relay.example"]);
    // ONE filter object — the actual SimplePool.subscribeMany signature (the
    // earlier sketch wrongly passed an array; live wiring caught it).
    expect(fp.getFilters()).toEqual({ kinds: [GIFT_WRAP], "#p": [guardianPk] });
  });

  it("delivers a parsed status and drops junk; close() unsubscribes", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const seen: DeviceStatus[] = [];
    const fp = fakePool();
    const sub = subscribeStatus(fp.pool, {
      relays: ["wss://r"],
      guardianPubkey: guardianPk,
      guardianSk,
      onStatus: (s) => seen.push(s),
      // Pin the freshness clock to the fixtures' wrap timestamp (status.ts).
      now: () => 1_700_000_000,
    });

    const s = sample(getPublicKey(machineSk), "cd".repeat(32));
    fp.fire(giftWrapStatus(s, machineSk, guardianPk));
    expect(seen).toEqual([s]);

    // A wrap for a DIFFERENT recipient / garbage decrypts to nothing → dropped.
    fp.fire(giftWrapStatus(s, machineSk, getPublicKey(generateSecretKey())));
    expect(seen).toHaveLength(1);

    sub.close();
    expect(fp.isClosed()).toBe(true);
  });
});
