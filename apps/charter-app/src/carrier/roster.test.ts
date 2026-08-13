import { describe, expect, it } from "vitest";
import { buildCarrierRoster } from "./roster";
import type { Child, Device } from "../domain/types";

function device(overrides: Partial<Device> = {}): Device {
  return {
    id: "dev-1",
    label: "Pixel 6",
    platform: "android",
    pairing: "paired",
    devicePubkey: "a".repeat(64),
    ...overrides,
  };
}

function child(overrides: Partial<Child> = {}): Child {
  return {
    id: "child-1",
    name: "Mia",
    color: "#ff0000",
    dependantPubkey: null,
    devices: [],
    policies: [],
    ...overrides,
  };
}

describe("buildCarrierRoster", () => {
  it("yields two entries for a child with two devices", () => {
    const c = child({
      devices: [
        device({ id: "d1", label: "Pixel 6", devicePubkey: "a".repeat(64) }),
        device({ id: "d2", label: "School laptop", devicePubkey: "b".repeat(64) }),
      ],
    });
    const roster = buildCarrierRoster([c]);
    expect(roster).toEqual([
      { machine: "a".repeat(64), childName: "Mia", deviceLabel: "Pixel 6" },
      { machine: "b".repeat(64), childName: "Mia", deviceLabel: "School laptop" },
    ]);
  });

  it("yields no entries for a child with no devices", () => {
    expect(buildCarrierRoster([child({ devices: [] })])).toEqual([]);
  });

  it("includes an unpaired device (has a pubkey but pairing !== 'paired')", () => {
    const c = child({
      devices: [device({ pairing: "unpaired", devicePubkey: "c".repeat(64) })],
    });
    expect(buildCarrierRoster([c])).toEqual([
      { machine: "c".repeat(64), childName: "Mia", deviceLabel: "Pixel 6" },
    ]);
  });

  it("skips a device with no pubkey yet — it has no machine identity to key on", () => {
    const c = child({ devices: [device({ devicePubkey: null })] });
    expect(buildCarrierRoster([c])).toEqual([]);
  });

  it("skips a device with an empty-string pubkey the same way", () => {
    const c = child({ devices: [device({ devicePubkey: "" })] });
    expect(buildCarrierRoster([c])).toEqual([]);
  });

  it("covers every child, not just the first", () => {
    const roster = buildCarrierRoster([
      child({ id: "c1", name: "Mia", devices: [device({ devicePubkey: "a".repeat(64) })] }),
      child({ id: "c2", name: "Robin", devices: [device({ devicePubkey: "b".repeat(64), label: "Old phone" })] }),
    ]);
    expect(roster.map((r) => r.childName)).toEqual(["Mia", "Robin"]);
  });

  it("an empty children list yields an empty roster", () => {
    expect(buildCarrierRoster([])).toEqual([]);
  });
});
