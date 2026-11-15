import { describe, expect, it } from "vitest";
import { UNNAMED_CHILD, UNNAMED_DEVICE, buildCarrierRoster } from "./roster";
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
      { machine: "a".repeat(64), childName: "Mia", deviceLabel: "Pixel 6", paired: true },
      { machine: "b".repeat(64), childName: "Mia", deviceLabel: "School laptop", paired: true },
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
      { machine: "c".repeat(64), childName: "Mia", deviceLabel: "Pixel 6", paired: false },
    ]);
  });

  it("marks only a fully paired device as paired (the carrier admits only those at the handshake)", () => {
    const c = child({
      devices: [
        device({ id: "d1", pairing: "paired", devicePubkey: "a".repeat(64) }),
        device({ id: "d2", pairing: "pairing", devicePubkey: "b".repeat(64) }),
        device({ id: "d3", pairing: "unpaired", devicePubkey: "c".repeat(64) }),
      ],
    });
    expect(buildCarrierRoster([c]).map((e) => e.paired)).toEqual([true, false, false]);
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

describe("buildCarrierRoster during the short grace after a release", () => {
  const machine = "c".repeat(64);
  const releasedList = (releasedAt: number) => [
    { machine, childId: "child-1", childName: "Mia", deviceLabel: "Pixel 6", relays: [], releasedAt },
  ];

  it("lists the machine as paired, not released, while it is releasing", () => {
    const now = Date.now();
    expect(buildCarrierRoster([child()], releasedList(now), now, new Set([machine]))).toEqual([
      { machine, childName: "Mia", deviceLabel: "Pixel 6", paired: true },
    ]);
  });

  it("says released again once the grace is over", () => {
    const now = Date.now();
    expect(buildCarrierRoster([], releasedList(now), now, new Set())).toEqual([
      { machine, childName: "Mia", deviceLabel: "Pixel 6", paired: false, released: true, releasedAt: Math.floor(now / 1000) },
    ]);
  });

  it("never lists a machine that is not on the released record as paired", () => {
    expect(buildCarrierRoster([], undefined, Date.now(), new Set([machine]))).toEqual([]);
  });
});

describe("buildCarrierRoster never pushes an empty name or label", () => {
  const machine = "c".repeat(64);

  it("falls back for a household child or device with no name, however blank", () => {
    const roster = buildCarrierRoster([
      child({ name: "", devices: [device({ label: "", devicePubkey: "a".repeat(64) })] }),
      child({ id: "c2", name: "   ", devices: [device({ id: "d2", label: " \t", devicePubkey: "b".repeat(64) })] }),
    ]);
    expect(roster.map((e) => [e.childName, e.deviceLabel])).toEqual([
      [UNNAMED_CHILD, UNNAMED_DEVICE],
      [UNNAMED_CHILD, UNNAMED_DEVICE],
    ]);
  });

  it("falls back for a released record that has lost its name or label (the push is never refused for it)", () => {
    const now = Date.now();
    const rec = (over: object) => [
      { machine, childId: "c", childName: "Mia", deviceLabel: "Pixel 6", relays: [], releasedAt: now - 5000, ...over },
    ];
    for (const releasing of [new Set<string>(), new Set([machine])]) {
      const got = buildCarrierRoster([], rec({ childName: "", deviceLabel: "" }), now, releasing);
      expect(got).toHaveLength(1);
      expect(got[0].childName).toBe(UNNAMED_CHILD);
      expect(got[0].deviceLabel).toBe(UNNAMED_DEVICE);
    }
    // A record with no name field at all is read as empty by the store, and gets the same fallback.
    const bare = [{ machine, childId: "c", releasedAt: now - 5000 }];
    expect(buildCarrierRoster([], bare, now)[0]).toMatchObject({
      childName: UNNAMED_CHILD,
      deviceLabel: "the device",
      released: true,
    });
  });

  it("keeps a real name and label exactly as they are", () => {
    expect(buildCarrierRoster([child({ devices: [device()] })])[0]).toMatchObject({ childName: "Mia", deviceLabel: "Pixel 6" });
  });

  it("puts releasedAt (whole seconds) on a released entry and on no other", () => {
    const now = 1_800_000_000_999;
    const roster = buildCarrierRoster(
      [child({ devices: [device()] })],
      [{ machine, childId: "c", childName: "Robin", deviceLabel: "Old", relays: [], releasedAt: now - 4000 }],
      now,
    );
    expect(roster[0]).not.toHaveProperty("releasedAt");
    expect(roster[1]).toMatchObject({ released: true, releasedAt: 1_799_999_996 });
    // During the short link grace it is listed as paired, with neither flag.
    const releasing = buildCarrierRoster([], [{ machine, childId: "c", childName: "Robin", deviceLabel: "Old", relays: [], releasedAt: now - 4000 }], now, new Set([machine]));
    expect(releasing[0]).not.toHaveProperty("released");
    expect(releasing[0]).not.toHaveProperty("releasedAt");
  });
});
