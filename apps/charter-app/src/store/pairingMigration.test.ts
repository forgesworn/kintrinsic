import { describe, expect, it } from "vitest";
import type { Child, Device } from "../domain/types";
import { DEFAULT_RELAYS, TROTTERS_RELAY } from "../signer/config";
import { backfillLegacyRelays } from "./pairingMigration";

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

describe("backfillLegacyRelays", () => {
  it("keeps the configured relay and adds independent routes once", () => {
    const device = dev({ relays: [TROTTERS_RELAY, "wss://family.example"] });
    const result = backfillLegacyRelays([child({ devices: [device] })]);
    expect(result.changed).toBe(true);
    expect(result.children[0].devices[0]).toEqual({
      ...device, relays: [TROTTERS_RELAY, "wss://family.example", ...DEFAULT_RELAYS], relaysUpdatedAt: 0,
    });
    expect(backfillLegacyRelays(result.children).changed).toBe(false);
  });

  it("never removes a relay explicitly added after the one-time repair", () => {
    for (const relaysUpdatedAt of [0, 123]) {
      const devices = [dev({ relays: [TROTTERS_RELAY, ...DEFAULT_RELAYS], relaysUpdatedAt })];
      const result = backfillLegacyRelays([child({ devices })]);
      expect(result.changed).toBe(false);
      expect(result.children[0].devices).toBe(devices);
    }
  });

  it("a legacy paired device with no relays is recovered onto redundant defaults", () => {
    const c = child({ devices: [dev({ id: "d1" })] });
    const { children, changed } = backfillLegacyRelays([c]);
    expect(changed).toBe(true);
    expect(children[0].devices[0].relays).toEqual([TROTTERS_RELAY, ...DEFAULT_RELAYS]);
  });

  it("never touches a device that already carries its own relays (new-style pairing)", () => {
    const own = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];
    const c = child({ devices: [dev({ id: "d1", relays: own })] });
    const { children, changed } = backfillLegacyRelays([c]);
    expect(changed).toBe(false);
    expect(children[0].devices[0].relays).toEqual(own);
    expect(children[0]).toBe(c); // untouched, same reference
  });

  it("leaves an unpaired or mid-pairing device alone — it has no relay to remember yet", () => {
    const c = child({
      devices: [dev({ id: "unpaired", pairing: "unpaired" }), dev({ id: "pairing", pairing: "pairing" })],
    });
    const { children, changed } = backfillLegacyRelays([c]);
    expect(changed).toBe(false);
    expect(children[0].devices[0].relays).toBeUndefined();
    expect(children[0].devices[1].relays).toBeUndefined();
  });

  it("is idempotent: running it again on its own output changes nothing further", () => {
    const c = child({ devices: [dev({ id: "d1" }), dev({ id: "d2", relays: ["wss://relay.example"] })] });
    const once = backfillLegacyRelays([c]);
    expect(once.changed).toBe(true);
    const twice = backfillLegacyRelays(once.children);
    expect(twice.changed).toBe(false);
    expect(twice.children).toEqual(once.children);
  });

  it("migrates across several children independently", () => {
    const a = child({ id: "a", devices: [dev({ id: "d1" })] });
    const b = child({ id: "b", devices: [dev({ id: "d2", relays: ["wss://relay.example"] })] });
    const { children, changed } = backfillLegacyRelays([a, b]);
    expect(changed).toBe(true);
    expect(children[0].devices[0].relays).toEqual([TROTTERS_RELAY, ...DEFAULT_RELAYS]);
    expect(children[1].devices[0].relays).toEqual(["wss://relay.example"]);
  });
});
