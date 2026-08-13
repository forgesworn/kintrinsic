import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { provisionCarrier, pushCarrierRoster, resetProvisionedForTests } from "./bridge";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import type { Child } from "../domain/types";

const KEY = "charter.guardian.key.v1";

beforeEach(() => {
  resetProvisionedForTests();
  resetGuardianKeyCacheForTests();
});

afterEach(() => {
  localStorage.clear();
  delete window.CharterCarrier;
  vi.restoreAllMocks();
});

describe("provisionCarrier", () => {
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
      { machine: "a".repeat(64), childName: "Mia", deviceLabel: "Pixel 6" },
    ]);
  });

  it("pushes an empty roster (not a no-op) when there is nothing to name yet", () => {
    const roster = vi.fn();
    window.CharterCarrier = { provision: vi.fn(), roster, isCarrier: () => true };

    expect(pushCarrierRoster([])).toBe(true);
    const payload = JSON.parse(roster.mock.calls[0][0]);
    expect(payload.entries).toEqual([]);
  });
});
