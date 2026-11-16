import { describe, expect, it } from "vitest";
import { reducer, type CharterState } from "./store";
import {
  RELEASED_GRACE_MS,
  findReleased,
  liveReleased,
  pressOrder,
  pruneReleased,
  releasedOf,
  releasedRelays,
  withReleased,
  withoutExpiredReleased,
  withoutReleased,
} from "./releasedDevices";
import { buildCarrierRoster } from "../carrier/roster";

const A = "a".repeat(64);
const B = "b".repeat(64);
const NOW = 1_800_000_000_000;

const device = (id: string, pk: string | null, relays?: string[]) => ({
  id,
  label: `Phone ${id}`,
  platform: "android" as const,
  pairing: "paired" as const,
  devicePubkey: pk,
  relays,
});

const base = (): CharterState => ({
  children: [
    {
      id: "c1",
      name: "Mia",
      color: "#111",
      dependantPubkey: null,
      policies: [],
      devices: [device("d1", A, ["wss://one"]), device("d2", B)],
    },
  ],
  requests: [],
  activity: [],
  signer: { connected: false, kind: "local", autoSign: true },
});

describe("releasedOf", () => {
  it("an absent or mis-shaped field is an empty list", () => {
    expect(releasedOf(undefined)).toEqual([]);
    expect(releasedOf("x")).toEqual([]);
    expect(releasedOf({})).toEqual([]);
  });
  it("drops entries that are not records, and fills what a record lacks", () => {
    const got = releasedOf([null, 3, { machine: A }, { machine: A, childId: "c", releasedAt: NOW }]);
    expect(got).toEqual([{ machine: A, childId: "c", childName: "", deviceLabel: "the device", relays: [], releasedAt: NOW }]);
  });
});

describe("the grace period", () => {
  const rec = { machine: A, childId: "c", childName: "Mia", deviceLabel: "P", relays: ["wss://r"], releasedAt: NOW };
  it("is 14 days, inclusive", () => {
    expect(RELEASED_GRACE_MS).toBe(14 * 24 * 60 * 60 * 1000);
    expect(findReleased([rec], A, NOW + RELEASED_GRACE_MS)).toBeDefined();
    expect(findReleased([rec], A, NOW + RELEASED_GRACE_MS + 1)).toBeUndefined();
  });
  it("never honours a record dated in the future", () => {
    expect(liveReleased([{ ...rec, releasedAt: NOW + 10 * 60_000 }], NOW)).toEqual([]);
  });
  it("its relays are kept only while it holds", () => {
    expect(releasedRelays([rec], NOW)).toEqual(["wss://r"]);
    expect(releasedRelays([rec], NOW + RELEASED_GRACE_MS + 1)).toEqual([]);
  });
  it("recording prunes the expired and replaces an older record of the same machine", () => {
    const old = { ...rec, machine: B, releasedAt: NOW - RELEASED_GRACE_MS - 1 };
    const dup = { ...rec, childName: "Old name" };
    const next = withReleased([old, dup], { id: "c", name: "New name" }, [{ label: "P", devicePubkey: A, relays: undefined }], NOW);
    expect(next).toHaveLength(1);
    expect(next[0].childName).toBe("New name");
  });
  it("a device with no pubkey is never recorded", () => {
    expect(withReleased([], { id: "c", name: "Mia" }, [{ label: "P", devicePubkey: null }], NOW)).toEqual([]);
  });
  it("withoutReleased reports no change as undefined", () => {
    expect(withoutReleased([rec], B)).toBeUndefined();
    expect(withoutReleased([rec], A)).toEqual([]);
  });
});

describe("the reducer records a disconnect", () => {
  it("unpairing (key cleared) records the device", () => {
    const next = reducer(base(), { type: "SET_DEVICE_PAIRING", childId: "c1", deviceId: "d1", pairing: "unpaired", devicePubkey: null });
    expect(next.releasedDevices).toMatchObject([{ machine: A, childId: "c1", childName: "Mia", deviceLabel: "Phone d1", relays: ["wss://one"] }]);
  });
  it("a pairing change that keeps the key records nothing", () => {
    const next = reducer(base(), { type: "SET_DEVICE_PAIRING", childId: "c1", deviceId: "d1", pairing: "pairing" });
    expect(next.releasedDevices).toBeUndefined();
  });
  it("removing a device records it; removing one with no key does not", () => {
    expect(reducer(base(), { type: "REMOVE_DEVICE", childId: "c1", deviceId: "d1" }).releasedDevices).toMatchObject([{ machine: A }]);
    const s = base();
    s.children[0].devices[0].devicePubkey = null;
    expect(reducer(s, { type: "REMOVE_DEVICE", childId: "c1", deviceId: "d1" }).releasedDevices).toBeUndefined();
  });
  it("removing the child records every device that had a key, under the name the child had", () => {
    const next = reducer(base(), { type: "REMOVE_CHILD", childId: "c1" });
    expect(next.children).toHaveLength(0);
    expect(next.releasedDevices?.map((r) => [r.machine, r.childName, r.childId])).toEqual([
      [A, "Mia", "c1"],
      [B, "Mia", "c1"],
    ]);
  });
  it("pairing a key again forgets the record for that machine", () => {
    const gone = reducer(base(), { type: "SET_DEVICE_PAIRING", childId: "c1", deviceId: "d1", pairing: "unpaired", devicePubkey: null });
    const back = reducer(gone, { type: "SET_DEVICE_PAIRING", childId: "c1", deviceId: "d1", pairing: "paired", devicePubkey: A });
    expect(back.releasedDevices).toEqual([]);
  });
  it("removing a child that does not exist changes nothing about the list", () => {
    expect(reducer(base(), { type: "REMOVE_CHILD", childId: "nope" }).releasedDevices).toBeUndefined();
  });
});

describe("the roster names a released device but never as paired", () => {
  const rec = { machine: A, childId: "c1", childName: "Mia", deviceLabel: "Phone d1", relays: [], releasedAt: NOW };
  it("adds it as unpaired and released", () => {
    expect(buildCarrierRoster([], [rec], NOW)).toEqual([
      { machine: A, childName: "Mia", deviceLabel: "Phone d1", paired: false, released: true, releasedAt: Math.floor(NOW / 1000) },
    ]);
  });
  it("a machine in the household again takes its household entry, once", () => {
    const roster = buildCarrierRoster(base().children, [rec], NOW);
    expect(roster.filter((e) => e.machine === A)).toEqual([
      { machine: A, childName: "Mia", deviceLabel: "Phone d1", paired: true },
    ]);
  });
  it("an ordinary entry carries no `released` key at all (an older carrier is unaffected)", () => {
    const roster = buildCarrierRoster(base().children, [], NOW);
    expect(roster.every((e) => !("released" in e))).toBe(true);
  });
});

describe("pressOrder", () => {
  const releasedAtMs = 1_800_000_000_500;
  const releasedAtSecs = 1_800_000_000;
  it("a stamped press at or before the disconnect is before it, a later one after", () => {
    expect(pressOrder({ at: releasedAtSecs - 60, atStamped: true }, releasedAtMs)).toBe("before");
    expect(pressOrder({ at: releasedAtSecs, atStamped: true }, releasedAtMs)).toBe("before");
    expect(pressOrder({ at: releasedAtSecs + 1, atStamped: true }, releasedAtMs)).toBe("after");
  });
  it("claims no order without a press time of the ward's own", () => {
    expect(pressOrder({ at: releasedAtSecs + 60, atStamped: false }, releasedAtMs)).toBe("unknown");
    expect(pressOrder({ at: releasedAtSecs - 60 }, releasedAtMs)).toBe("unknown");
    expect(pressOrder({ at: 0, atStamped: true }, releasedAtMs)).toBe("unknown");
    expect(pressOrder({ at: Number.NaN, atStamped: true }, releasedAtMs)).toBe("unknown");
    expect(pressOrder({ at: releasedAtSecs, atStamped: true }, Number.NaN)).toBe("unknown");
  });
});

describe("pruneReleased", () => {
  const live = { machine: A, childId: "c1", childName: "Mia", deviceLabel: "Phone", relays: [], releasedAt: NOW - 1000 };
  const old = { machine: B, childId: "c2", childName: "Robin", deviceLabel: "Old", relays: [], releasedAt: NOW - RELEASED_GRACE_MS - 1000 };
  const state = (list?: unknown) => ({ ...base(), ...(list === undefined ? {} : { releasedDevices: list as never }) });

  it("drops an expired record and keeps a live one", () => {
    const pruned = pruneReleased(state([old, live]), NOW);
    expect(pruned.releasedDevices).toEqual([live]);
  });
  it("returns the same object when nothing expired, or there is no list", () => {
    const s = state([live]);
    expect(pruneReleased(s, NOW)).toBe(s);
    const none = state();
    expect(pruneReleased(none, NOW)).toBe(none);
  });
  it("a list whose records are all expired becomes an empty one, with no name left", () => {
    const pruned = pruneReleased(state([old]), NOW);
    expect(pruned.releasedDevices).toEqual([]);
    expect(JSON.stringify(pruned)).not.toContain("Robin");
  });
  it("a mis-shaped list is replaced by an empty one", () => {
    expect(pruneReleased(state("nonsense"), NOW).releasedDevices).toEqual([]);
  });

  describe("withoutExpiredReleased (the backup)", () => {
    it("prunes the JSON blob", () => {
      const out = withoutExpiredReleased(JSON.stringify(state([old, live])), NOW);
      expect(JSON.parse(out).releasedDevices).toEqual([live]);
      expect(out).not.toContain("Robin");
    });
    it("returns the very same text when nothing expired, and passes anything unreadable through", () => {
      const raw = JSON.stringify(state([live]));
      expect(withoutExpiredReleased(raw, NOW)).toBe(raw);
      expect(withoutExpiredReleased("{not json", NOW)).toBe("{not json");
      expect(withoutExpiredReleased("[1]", NOW)).toBe("[1]");
    });
  });
});
