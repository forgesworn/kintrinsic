import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { provisionCarrier, pushCarrierRoster, resetProvisionedForTests } from "./bridge";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import { clearPhoneWorkingKey, resetPhoneWorkingKeyForTests } from "../signer/phoneWorkingKey";
import type { Child } from "../domain/types";

const KEY = "charter.guardian.key.v1";
const WORKING = "charter.guardian.working.v1";

const bytesHex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
const hexBytes = (h: string) => Uint8Array.from(h.match(/../g)!.map((x) => parseInt(x, 16)));

beforeEach(() => {
  resetProvisionedForTests();
  resetGuardianKeyCacheForTests();
  resetPhoneWorkingKeyForTests();
});

afterEach(() => {
  localStorage.clear();
  delete window.CharterCarrier;
  vi.restoreAllMocks();
});

describe("provisionCarrier", () => {
  it("updates relay routing when a paired device's list changes without provisioning on every render", () => {
    localStorage.setItem(KEY, nip19.nsecEncode(generateSecretKey()));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    const children: Child[] = [{ id: "c", name: "Robin", color: "#abc", dependantPubkey: null, policies: [],
      devices: [{ id: "d", label: "laptop", platform: "linux", pairing: "paired", relays: ["wss://relay.trotters.cc"] }] }];
    provisionCarrier(children);
    provisionCarrier(children);
    expect(provision).toHaveBeenCalledTimes(1);
    expect(JSON.parse(provision.mock.calls[0][0]).relays).toContain("wss://relay.trotters.cc");
    children[0].devices[0].relays = ["wss://family.example"];
    provisionCarrier(children);
    expect(provision).toHaveBeenCalledTimes(2);
    expect(JSON.parse(provision.mock.calls[1][0]).relays).toContain("wss://family.example");
    expect(JSON.parse(provision.mock.calls[1][0]).relays).not.toContain("wss://relay.trotters.cc");
  });
  it("is a no-op outside the carrier", () => {
    localStorage.setItem(KEY, nip19.nsecEncode(generateSecretKey()));
    expect(provisionCarrier()).toBe(false);
  });

  it("hands the key + relays to the shell inside the carrier", () => {
    const sk = generateSecretKey();
    localStorage.setItem(KEY, nip19.nsecEncode(sk));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };

    expect(provisionCarrier()).toBe(true);
    expect(provision).toHaveBeenCalledTimes(1);
    const payload = JSON.parse(provision.mock.calls[0][0]);
    expect(payload.v).toBe(1);
    expect(payload.guardianSkHex).toMatch(/^[0-9a-f]{64}$/);
    expect(payload.guardianPubkeyHex).toBe(getPublicKey(sk));
    expect(payload.relays.length).toBeGreaterThan(0);
    expect(payload.relays[0]).toMatch(/^wss:\/\//);
  });

  /*
   * S4 (review 2026-08-07). `App` calls this from an unconditional effect so a
   * restore-from-backup is caught without a remount — which meant serialising
   * the family's root signing key to hex and pushing it over the JS bridge on
   * EVERY render. The effect stays unconditional; the hand-off does not.
   */
  it("hands the key over ONCE, not on every render", () => {
    localStorage.setItem(KEY, nip19.nsecEncode(generateSecretKey()));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };

    for (let i = 0; i < 25; i++) expect(provisionCarrier()).toBe(true);
    expect(provision).toHaveBeenCalledTimes(1);
  });

  // …but a key that actually CHANGES must still get through, or a restore
  // would leave the native service listening with the old identity forever.
  it("hands over again when the key changes", () => {
    localStorage.setItem(KEY, nip19.nsecEncode(generateSecretKey()));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    provisionCarrier();
    provisionCarrier();

    const restored = generateSecretKey();
    resetGuardianKeyCacheForTests();
    localStorage.setItem(KEY, nip19.nsecEncode(restored));
    expect(provisionCarrier()).toBe(true);

    expect(provision).toHaveBeenCalledTimes(2);
    expect(JSON.parse(provision.mock.calls[1][0]).guardianPubkeyHex).toBe(
      getPublicKey(restored),
    );
  });

  it("lists the guardian key alone in keys when this phone has no working key", () => {
    const sk = generateSecretKey();
    localStorage.setItem(KEY, nip19.nsecEncode(sk));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    provisionCarrier();
    const payload = JSON.parse(provision.mock.calls[0][0]);
    expect(payload.keys).toEqual([{ skHex: bytesHex(sk), pubkeyHex: getPublicKey(sk) }]);
  });

  /*
   * D5: a phone-held root R that minted a working key W gets ward traffic on
   * both until every ward has taken W, so the carrier is handed both — and
   * handed again whenever W is minted or goes.
   */
  it("hands over the working key beside the guardian key, and again when it is minted or cleared", () => {
    const r = generateSecretKey();
    localStorage.setItem(KEY, nip19.nsecEncode(r));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    provisionCarrier();
    expect(JSON.parse(provision.mock.calls[0][0]).keys).toHaveLength(1);

    const w = generateSecretKey();
    localStorage.setItem(WORKING, nip19.nsecEncode(w));
    resetPhoneWorkingKeyForTests();
    provisionCarrier();
    provisionCarrier();
    expect(provision).toHaveBeenCalledTimes(2);
    const both = JSON.parse(provision.mock.calls[1][0]);
    // No device lists W yet: the primary is still R.
    expect(both.guardianPubkeyHex).toBe(getPublicKey(r));
    expect(both.keys).toEqual([
      { skHex: bytesHex(r), pubkeyHex: getPublicKey(r) },
      { skHex: bytesHex(w), pubkeyHex: getPublicKey(w) },
    ]);
    for (const k of both.keys) expect(getPublicKey(hexBytes(k.skHex))).toBe(k.pubkeyHex);

    // W promoted into the guardian key slot and its own slot cleared
    // (retirePhoneKey): W alone.
    localStorage.setItem(KEY, nip19.nsecEncode(w));
    resetGuardianKeyCacheForTests();
    clearPhoneWorkingKey();
    provisionCarrier();
    expect(provision).toHaveBeenCalledTimes(3);
    const promoted = JSON.parse(provision.mock.calls[2][0]);
    expect(promoted.guardianPubkeyHex).toBe(getPublicKey(w));
    expect(promoted.keys).toEqual([{ skHex: bytesHex(w), pubkeyHex: getPublicKey(w) }]);
  });

  it("makes W the primary once a device lists it, still listing both, and moves back when no device does", () => {
    const r = generateSecretKey();
    const w = generateSecretKey();
    localStorage.setItem(KEY, nip19.nsecEncode(r));
    localStorage.setItem(WORKING, nip19.nsecEncode(w));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    const device = { id: "d", label: "phone", platform: "android" as const, pairing: "paired" as const };
    const listing = (keys: string[]): Child[] => [{ id: "c", name: "Robin", color: "#abc", dependantPubkey: null, policies: [],
      devices: [{ ...device, guardians: { roots: [{ root: getPublicKey(r), keys }], pending: [], relays: [] } as unknown as Child["devices"][number]["guardians"] }] }];

    provisionCarrier(listing([getPublicKey(w)]));
    const first = JSON.parse(provision.mock.calls[0][0]);
    expect(first.guardianPubkeyHex).toBe(getPublicKey(w));
    expect(first.guardianSkHex).toBe(bytesHex(w));
    expect(first.keys.map((k: { pubkeyHex: string }) => k.pubkeyHex)).toEqual([getPublicKey(r), getPublicKey(w)]);

    // W withdrawn: the devices stop listing it, and the primary goes back to R.
    provisionCarrier(listing([]));
    expect(provision).toHaveBeenCalledTimes(2);
    expect(JSON.parse(provision.mock.calls[1][0]).guardianPubkeyHex).toBe(getPublicKey(r));
  });

  it("does not hand off when no key exists yet (absent) or the key is corrupt", () => {
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    expect(provisionCarrier()).toBe(false); // absent — never mint from the bridge

    localStorage.setItem(KEY, "garbage-not-an-nsec");
    expect(provisionCarrier()).toBe(false); // corrupt — restore flow owns this
    expect(provision).not.toHaveBeenCalled();
  });
});

const CHILDREN: Child[] = [
  {
    id: "child-1",
    name: "Mia",
    color: "#ff0000",
    dependantPubkey: null,
    policies: [],
    devices: [
      {
        id: "dev-1",
        label: "Pixel 6",
        platform: "android",
        pairing: "paired",
        devicePubkey: "a".repeat(64),
      },
    ],
  },
];

describe("pushCarrierRoster", () => {
  it("is a no-op outside the carrier", () => {
    expect(pushCarrierRoster(CHILDREN)).toBe(false);
  });

  it("is a no-op against an older shell that has no roster method", () => {
    // Provisioning is not gated on this — an APK shipped before the roster
    // feature must not throw when the PWA in front of it is newer.
    window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true };
    expect(pushCarrierRoster(CHILDREN)).toBe(false);
  });

  it("hands the roster to the shell inside the carrier", () => {
    const roster = vi.fn();
    window.CharterCarrier = { provision: vi.fn(), roster, isCarrier: () => true };

    expect(pushCarrierRoster(CHILDREN)).toBe(true);
    expect(roster).toHaveBeenCalledTimes(1);
    const payload = JSON.parse(roster.mock.calls[0][0]);
    expect(payload.v).toBe(1);
    expect(payload.entries).toEqual([
      { machine: "a".repeat(64), childName: "Mia", deviceLabel: "Pixel 6", paired: true },
    ]);
  });

  it("tells the carrier which machines are paired, so only those are admitted over the home network", () => {
    const roster = vi.fn();
    window.CharterCarrier = { provision: vi.fn(), roster, isCarrier: () => true };
    const mixed: Child[] = [
      {
        ...CHILDREN[0],
        devices: [
          { ...CHILDREN[0].devices[0], id: "p", devicePubkey: "a".repeat(64), pairing: "paired" },
          { ...CHILDREN[0].devices[0], id: "q", devicePubkey: "b".repeat(64), pairing: "pairing" },
        ],
      },
    ];
    expect(pushCarrierRoster(mixed)).toBe(true);
    const entries = JSON.parse(roster.mock.calls[0][0]).entries as { machine: string; paired: boolean }[];
    expect(entries.map((e) => [e.machine[0], e.paired])).toEqual([["a", true], ["b", false]]);
  });

  it("pushes an empty roster (not a no-op) when there is nothing to name yet", () => {
    const roster = vi.fn();
    window.CharterCarrier = { provision: vi.fn(), roster, isCarrier: () => true };

    expect(pushCarrierRoster([])).toBe(true);
    const payload = JSON.parse(roster.mock.calls[0][0]);
    expect(payload.entries).toEqual([]);
  });
});
