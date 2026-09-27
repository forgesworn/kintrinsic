import { describe, expect, it } from "vitest";
import type { Child, Device } from "../domain/types";
import { LEGACY_PAIRING_RELAYS } from "./config";
import { allPairingRelays, deviceRelays } from "./pairingRelays";

const DEFAULTS = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];

const dev = (over: Partial<Device>): Device => ({
  id: "d",
  label: "laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: "f".repeat(64),
  ...over,
});
const child = (over: Partial<Child>): Child => ({
  id: "child_sam",
  name: "Sam",
  color: "#abc",
  dependantPubkey: null,
  devices: [],
  policies: [],
  ...over,
});

describe("deviceRelays", () => {
  it("uses the device's own relays when it has them", () => {
    expect(deviceRelays(dev({ relays: LEGACY_PAIRING_RELAYS }), DEFAULTS)).toEqual(LEGACY_PAIRING_RELAYS);
  });

  it("falls back to the defaults when the device has none", () => {
    expect(deviceRelays(dev({}), DEFAULTS)).toEqual(DEFAULTS);
  });
});

describe("allPairingRelays", () => {
  it("a legacy pairing (relays = trotters, back-filled) keeps being polled on trotters", () => {
    const c = child({ devices: [dev({ relays: LEGACY_PAIRING_RELAYS })] });
    const relays = allPairingRelays([c], DEFAULTS);
    expect(relays).toContain("wss://relay.trotters.cc");
    // ...and the current defaults, so it's not the ONLY relay covered.
    for (const r of DEFAULTS) expect(relays).toContain(r);
  });

  it("the union covers two devices paired on different relays", () => {
    const a = child({
      id: "a",
      devices: [dev({ id: "d1", relays: ["wss://relay.trotters.cc"] })],
    });
    const b = child({
      id: "b",
      devices: [dev({ id: "d2", relays: ["wss://relay.example"] })],
    });
    const relays = allPairingRelays([a, b], DEFAULTS);
    expect(relays).toContain("wss://relay.trotters.cc");
    expect(relays).toContain("wss://relay.example");
    for (const r of DEFAULTS) expect(relays).toContain(r);
  });

  it("a device with no relays at all (mid-pairing, or unmigrated) contributes only the defaults", () => {
    const c = child({ devices: [dev({ pairing: "pairing", relays: undefined })] });
    expect(allPairingRelays([c], DEFAULTS)).toEqual(DEFAULTS);
  });

  it("ignores an unpaired device's stale relays", () => {
    const c = child({ devices: [dev({ pairing: "unpaired", relays: ["wss://relay.stale"] })] });
    const relays = allPairingRelays([c], DEFAULTS);
    expect(relays).not.toContain("wss://relay.stale");
  });

  it("with no legacy or custom relays anywhere, it is exactly the defaults", () => {
    const c = child({ devices: [dev({})] });
    expect(allPairingRelays([c], DEFAULTS)).toEqual(DEFAULTS);
  });
});
