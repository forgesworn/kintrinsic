// The household side of guardian keys: the per-device view a STATUS leaves,
// a departed key's last STATUS (the farewell), roster ids, and the guard on
// connecting My Signet over a family whose devices trust this phone's key.

import { describe, expect, it } from "vitest";
import { finalizeEvent, generateSecretKey, getEventHash, getPublicKey, nip44, type NostrEvent } from "nostr-tools";
import type { Child, Device } from "../domain/types";
import { applyDeviceKeysView, applyFarewell, devicesPinningLocalKey, ensureRosterIds, keysChildren } from "./guardianKeysWiring";
import { parseFarewell, parseStatus, unwrapFarewell, unwrapStatus, type DeviceStatus } from "../wire/status";
import { ingestWrap, type WrapIngestContext } from "./ingestWrap";
import { resolveChildTarget } from "../signer/resolveChild";

const wSk = generateSecretKey();
const W = getPublicKey(wSk);
const R = "a1".repeat(32);
const R2 = "a2".repeat(32);
const machineSk = generateSecretKey();
const M = getPublicKey(machineSk);
const T = Math.floor(Date.now() / 1000);

function device(extra: Partial<Device> = {}): Device {
  return { id: "d1", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: M, ...extra };
}
function child(devices: Device[], extra: Partial<Child> = {}): Child {
  return { id: "c1", name: "Sam", color: "#000", dependantPubkey: null, devices, policies: [], ...extra };
}

const fullStatus = (ts: number, guardians?: DeviceStatus["guardians"], appVersionCode = 49): DeviceStatus =>
  parseStatus(
    JSON.stringify({
      v: 1, subject: M, machine: M, ts, dayKey: "d", usedTodaySecs: 5, windowLeftSecs: 0, quotaLeftSecs: 0,
      effectiveSecs: 0, locked: false, source: "guardian", appVersionCode, ...(guardians ? { guardians } : {}),
    }),
  )!;

const g = (roots: { root: string; keys: string[] }[]) => ({ roots, pending: [], tombstones: 0, relays: ["wss://x.example"] });

/** A ward-sealed STATUS wrap of `content` to `to`, as charterd builds it. */
function wardWrap(content: object, to: string): NostrEvent {
  const rumor = { kind: 31114, created_at: T, tags: [], content: JSON.stringify(content), pubkey: M } as unknown as NostrEvent;
  rumor.id = getEventHash(rumor);
  const seal = finalizeEvent(
    { kind: 13, created_at: T, tags: [], content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, to)) },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    { kind: 1059, created_at: T, tags: [["p", to]], content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, to)) },
    eph,
  );
}

const farewellBody = (roots: { root: string; keys: string[] }[], ts = T) => ({
  v: 1,
  machine: M,
  ts,
  seq: 42,
  guardians: { roots, pending: [{ d: `kintrinsic/unguardian/${R}`, kind: "removal", takesEffectAt: ts, expiresAt: ts + 9 }], tombstones: 3 },
});

describe("device keys view", () => {
  it("records version and guardians from a newer STATUS only", () => {
    const kids = [child([device()])];
    const a = applyDeviceKeysView(kids, fullStatus(100, g([{ root: R, keys: [W] }])));
    expect(a[0].devices[0]).toMatchObject({ appVersionCode: 49, keysViewAt: 100, guardians: g([{ root: R, keys: [W] }]) });
    // Same again: no churn.
    expect(applyDeviceKeysView(a, fullStatus(100, g([{ root: R, keys: [W] }])))).toBe(a);
    // Older: ignored.
    expect(applyDeviceKeysView(a, fullStatus(50, undefined, 1))).toBe(a);
    // Newer without guardians: an older ward again (rolled back); the view follows it.
    const b = applyDeviceKeysView(a, fullStatus(200, undefined, 48));
    expect(b[0].devices[0].guardians).toBeUndefined();
    expect(b[0].devices[0].appVersionCode).toBe(48);
  });

  it("feeds the gate: statusSeen from a STATUS read here", () => {
    const view = keysChildren([child([device(), device({ id: "d2", devicePubkey: "b".repeat(64), lastSeenAt: 1 })])], ["wss://d.example"]);
    expect(view[0].devices.map((d) => d.statusSeen)).toEqual([false, true]);
  });
});

describe("a departed key's last STATUS (farewell)", () => {
  it("parses the minimal shape and never as a full STATUS", () => {
    const body = farewellBody([{ root: R2, keys: [] }]);
    expect(parseStatus(JSON.stringify(body))).toBeNull();
    const f = parseFarewell(JSON.stringify(body))!;
    expect(f).toMatchObject({ machine: M, ts: T, seq: 42 });
    expect(f.guardians.relays).toEqual([]);
    expect(f.guardians.pending[0].kind).toBe("removal");
    // A full STATUS isn't a farewell.
    expect(parseFarewell(JSON.stringify({ ...body, subject: M }))).toBeNull();
    expect(parseFarewell(JSON.stringify({ ...body, guardians: { roots: 1 } }))).toBeNull();
  });

  it("is authenticated like a STATUS: sealed by the machine it names", () => {
    const wrap = wardWrap(farewellBody([{ root: R2, keys: [] }]), W);
    expect(unwrapStatus(wrap, wSk)).toBeNull();
    expect(unwrapFarewell(wrap, wSk)?.machine).toBe(M);
    const lying = wardWrap({ ...farewellBody([]), machine: "c".repeat(64) }, W);
    expect(unwrapFarewell(lying, wSk)).toBeNull();
  });

  it("marks the device departed when W is gone, and nothing more is sent to it", () => {
    const kids = [child([device({ keysViewAt: T - 10, guardians: g([{ root: R, keys: [W] }]) })])];
    const f = parseFarewell(JSON.stringify(farewellBody([{ root: R2, keys: [] }])))!;
    const after = applyFarewell(kids, f, W);
    expect(after[0].devices[0]).toMatchObject({ departedAt: T, keysViewAt: T });
    expect(after[0].devices[0].guardians?.roots).toEqual([{ root: R2, keys: [] }]);
    // Never usage: lastSeenAt untouched.
    expect(after[0].devices[0].lastSeenAt).toBeUndefined();
    expect(resolveChildTarget(after, "c1", ["wss://d.example"])?.devices).toEqual([]);
    expect(keysChildren(after, [])[0].devices).toEqual([]);
    // A later full STATUS to this install clears it.
    const back = applyDeviceKeysView(after, fullStatus(T + 5, g([{ root: R, keys: [W] }])));
    expect(back[0].devices[0].departedAt).toBeUndefined();
  });

  it("ignores a farewell that still lists W, or one older than the view held", () => {
    const kids = [child([device({ keysViewAt: T })])];
    expect(applyFarewell(kids, parseFarewell(JSON.stringify(farewellBody([{ root: R, keys: [W] }])))!, W)).toBe(kids);
    expect(applyFarewell(kids, parseFarewell(JSON.stringify(farewellBody([], T - 1)))!, W)).toBe(kids);
  });

  it("routes through the shared intake, never as a status", () => {
    const seen: string[] = [];
    const ctx: WrapIngestContext = {
      guardianSk: wSk,
      handledWraps: new Set(),
      seenReqIds: new Set(),
      hasRequest: () => false,
      ingestStatus: () => seen.push("status"),
      ingestRequest: () => true,
      ingestFarewell: (f) => seen.push(`farewell:${f.machine}`),
    };
    expect(ingestWrap(wardWrap(farewellBody([]), W), ctx)).toEqual({ kind: "farewell" });
    expect(seen).toEqual([`farewell:${M}`]);
  });
});

describe("roster ids", () => {
  it("mints one per child once", () => {
    const kids = [child([]), child([], { id: "c2", rosterId: "ab".repeat(32) })];
    const { children, changed } = ensureRosterIds(kids, () => "cd".repeat(32));
    expect(changed).toBe(true);
    expect(children.map((c) => c.rosterId)).toEqual(["cd".repeat(32), "ab".repeat(32)]);
    expect(ensureRosterIds(children)).toEqual({ children, changed: false });
  });
});

describe("connecting My Signet over a family that trusts this phone's key", () => {
  it("names devices heard from with the local key, unless they hold it as a working key", () => {
    const heard = device({ lastSeenAt: 1 });
    const working = device({ id: "d2", label: "Laptop", devicePubkey: "b".repeat(64), keysViewAt: 1, guardians: g([{ root: R, keys: [W] }]) });
    const silent = device({ id: "d3", label: "Tablet", devicePubkey: "e".repeat(64) });
    expect(devicesPinningLocalKey([child([heard, working, silent])], W)).toEqual(["Sam's phone"]);
  });
});
