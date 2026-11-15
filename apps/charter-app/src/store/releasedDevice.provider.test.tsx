// A press made just before the guardian disconnected a phone still reaches
// Activity. When a guardian unpairs a device, removes it, or removes its child,
// the ward answers the release by sending every audit it was holding, so the
// press arrives AFTER the console has forgotten the device. For 14 days the
// console remembers it, and lands exactly two kinds of audit from it (an
// emergency unlock and the emergency-call button), with a line that says the
// press was made before the disconnect. Nothing else from it is honoured.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import { emergencyCallTags, overrideTags, overrideWrap } from "../test/overrideWrap";
import { CHARTER_DEVICE_REQUEST, type DeviceRequest } from "../wire/request";

const relayHolds: { wraps: NostrEvent[]; asked: string[][] } = { wraps: [], asked: [] };
vi.mock("nostr-tools", async (importOriginal) => {
  const real = await importOriginal<typeof import("nostr-tools")>();
  class FakePool {
    async querySync(relays: string[]): Promise<NostrEvent[]> {
      relayHolds.asked.push(relays);
      return relayHolds.wraps;
    }
    publish(): Promise<string>[] {
      return [];
    }
    close(): void {}
  }
  return { ...real, SimplePool: FakePool };
});

import { CharterProvider, STORAGE_KEY, useCharter, type CharterState } from "./store";
import { RELEASED_GRACE_MS, type ReleasedDevice } from "./releasedDevices";
import { buildCarrierRoster } from "../carrier/roster";

const DAY = 24 * 60 * 60 * 1000;
const guardianSk = generateSecretKey();
const guardianPk = getPublicKey(guardianSk);
const machineSk = generateSecretKey();
const machinePk = getPublicKey(machineSk);
const OWN_RELAY = "wss://family.example";

let pressedAt: number; // unix seconds, set per test
let api: ReturnType<typeof useCharter>;

const callWrap = (at = pressedAt): NostrEvent =>
  overrideWrap({
    guardianPk,
    machineSk,
    tags: emergencyCallTags([["at", String(at)]]),
    createdAt: Math.floor(Date.now() / 1000),
  });

const unlockWrap = (at = pressedAt): NostrEvent =>
  overrideWrap({
    guardianPk,
    machineSk,
    tags: overrideTags([["at", String(at)]]),
    createdAt: Math.floor(Date.now() / 1000),
  });

function controlWrap(): NostrEvent {
  return overrideWrap({
    guardianPk,
    machineSk,
    tags: [["t", "charter-device"], ["outcome", "override"], ["op", "alarm.silence"], ["at", String(pressedAt)]],
    createdAt: Math.floor(Date.now() / 1000),
  });
}

/** An ask, built the way the ward builds it (rumor, machine-signed seal, throwaway wrap). */
function askWrap(): NostrEvent {
  const ts = Math.floor(Date.now() / 1000);
  const req: DeviceRequest = {
    v: 1,
    op: "time.extend",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: machinePk,
    ts,
    params: { minutesRequested: 30, reason: "homework", limitHit: "budget" },
  } as DeviceRequest;
  const rumor = {
    kind: CHARTER_DEVICE_REQUEST,
    pubkey: machinePk,
    created_at: ts,
    tags: [["t", "charter-device"]],
    content: JSON.stringify(req),
  };
  const seal = finalizeEvent(
    {
      kind: 13,
      created_at: ts,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, guardianPk)),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: 1059,
      created_at: ts,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

function seeded(extra: Partial<CharterState> = {}): CharterState {
  return {
    children: [
      {
        id: "c1",
        name: "Robin",
        color: "#3B6FB2",
        dependantPubkey: null,
        policies: [{ id: "p1", scope: { kind: "device" } }],
        devices: [
          {
            id: "d1",
            label: "Laptop",
            platform: "linux",
            pairing: "paired",
            devicePubkey: machinePk,
            relays: [OWN_RELAY],
          },
        ],
      },
    ],
    requests: [],
    activity: [],
    signer: { connected: false, kind: "local", autoSign: true },
    ...extra,
  };
}

function Probe() {
  api = useCharter();
  return <div data-testid="state">{JSON.stringify(api.state)}</div>;
}

describe("a press made before the guardian disconnected the device", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded()));
    relayHolds.wraps = [];
    relayHolds.asked = [];
    pressedAt = Math.floor(Date.now() / 1000) - 600;
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    delete window.CharterCarrier;
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    vi.useRealTimers();
  });

  function mount() {
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  }

  const current = () =>
    JSON.parse(container.querySelector('[data-testid="state"]')!.textContent!) as CharterState;
  type Line = { childId: string; ts: number; outcome: string; summary: string; auditKey?: string };
  const presses = (): Line[] =>
    current().activity.filter((l) => l.outcome !== "rule-changed") as Line[];
  const stored = () => JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;

  const settle = async (ms = 50) => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  };

  function bridge(wraps: NostrEvent[], onConsume: (ids: string[]) => void = () => {}) {
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
      localSend: () => JSON.stringify({ state: "unavailable" }),
      localDelivery: () => "{}",
      localInbox: () =>
        JSON.stringify(wraps.map((w) => ({ id: w.id, machine: machinePk, wrap: w, receivedAt: 1, verifiedAt: Math.floor(Date.now() / 1000) }))),
      localConsume: (json: string) => onConsume(JSON.parse(json) as string[]),
    };
  }

  describe("the provider test: unpair, then deliver the old machine's wrap", () => {
    it("unpair, then an emergency-call wrap from the old machine: exactly one Activity line, saying so", async () => {
      mount();
      await settle();
      act(() => api.unpairDevice("c1", "d1"));
      expect(current().children[0].devices[0].devicePubkey).toBeNull();

      relayHolds.wraps = [callWrap()];
      await settle(31_000);
      const got = presses();
      expect(got).toHaveLength(1);
      expect(got[0]).toMatchObject({
        childId: "c1",
        outcome: "emergency_call",
        ts: pressedAt * 1000,
        summary:
          "Used the emergency call button on Laptop, before you disconnected it from Robin. Kintrinsic can't see whether a call was made.",
      });
      // More polls, and a copy by the home network too: still one.
      bridge([callWrap()]);
      await settle(31_000);
      await settle(6_000);
      expect(presses()).toHaveLength(1);
    });

    it("an emergency unlock from the old machine lands too, with the same wording", async () => {
      mount();
      await settle();
      act(() => api.unpairDevice("c1", "d1"));
      relayHolds.wraps = [unlockWrap()];
      await settle(31_000);
      const got = presses();
      expect(got).toHaveLength(1);
      expect(got[0].outcome).toBe("override");
      expect(got[0].summary).toBe("Opened their phone on Laptop for 5 minutes, before you disconnected it from Robin");
    });
  });

  describe("after the child is removed", () => {
    it("removing the child, then a held press: one line, keeping the child's id and name, shown without the child", async () => {
      mount();
      await settle();
      act(() => api.removeChild("c1"));
      expect(current().children).toHaveLength(0);
      relayHolds.wraps = [callWrap()];
      await settle(31_000);
      const got = presses();
      expect(got).toHaveLength(1);
      expect(got[0].childId).toBe("c1");
      expect(got[0].summary).toContain("before you disconnected it from Robin");
      // It is in storage, so a reload keeps it.
      expect(stored().activity.some((l) => l.outcome === "emergency_call")).toBe(true);
    });

    it("removing just the device, then a held press: one line", async () => {
      mount();
      await settle();
      act(() => api.removeDevice("c1", "d1"));
      relayHolds.wraps = [callWrap()];
      await settle(31_000);
      expect(presses()).toHaveLength(1);
    });
  });

  describe("what is recorded", () => {
    it("records machine, child, name, label, relays and time, for every way of disconnecting", async () => {
      mount();
      await settle();
      const before = Date.now();
      act(() => api.unpairDevice("c1", "d1"));
      const rec = stored().releasedDevices as ReleasedDevice[];
      expect(rec).toHaveLength(1);
      expect(rec[0]).toMatchObject({
        machine: machinePk,
        childId: "c1",
        childName: "Robin",
        deviceLabel: "Laptop",
        relays: [OWN_RELAY],
      });
      expect(rec[0].releasedAt).toBeGreaterThanOrEqual(before);
    });

    it("a household saved by an older build has no list, and loads cleanly as an empty one", async () => {
      expect("releasedDevices" in JSON.parse(localStorage.getItem(STORAGE_KEY)!)).toBe(false);
      mount();
      await settle();
      expect(current().children).toHaveLength(1);
      expect(presses()).toHaveLength(0);
    });

    it("a mis-shaped list is treated as empty, not a crash", async () => {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...seeded(), releasedDevices: "nonsense" }));
      mount();
      await settle();
      act(() => api.unpairDevice("c1", "d1"));
      expect(stored().releasedDevices).toHaveLength(1);
    });
  });

  describe("what a released machine may NOT do", () => {
    const seededReleased = (releasedAt = Date.now() - 1000): CharterState =>
      seeded({
        children: [{ ...seeded().children[0], devices: [] }],
        releasedDevices: [
          { machine: machinePk, childId: "c1", childName: "Robin", deviceLabel: "Laptop", relays: [OWN_RELAY], releasedAt },
        ],
      });

    it("an unknown emergency control from it is not landed, and the carrier is not told to forget it", async () => {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(seededReleased()));
      const w = controlWrap();
      relayHolds.wraps = [w];
      const consume = vi.fn();
      bridge([w], consume);
      mount();
      await settle();
      await settle(6_000);
      expect(presses()).toHaveLength(0);
      expect(consume).not.toHaveBeenCalled();
    });

    it("an ask from it is not landed", async () => {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(seededReleased()));
      relayHolds.wraps = [askWrap()];
      mount();
      await settle();
      await settle(31_000);
      expect(current().requests).toHaveLength(0);
    });

    it("it is not paired for anything: no device, nothing to deliver to or attribute a status to", async () => {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(seededReleased()));
      mount();
      await settle();
      const s = current();
      expect(s.children.flatMap((c) => c.devices)).toHaveLength(0);
      expect(s.children.flatMap((c) => c.devices).filter((d) => d.pairing === "paired")).toHaveLength(0);
    });

    it("a record older than 14 days lands nothing", async () => {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(seededReleased(Date.now() - RELEASED_GRACE_MS - DAY)));
      relayHolds.wraps = [callWrap()];
      const consume = vi.fn();
      bridge([callWrap()], consume);
      mount();
      await settle();
      await settle(6_000);
      expect(presses()).toHaveLength(0);
    });

    it("a machine that was never in the household still lands nothing", async () => {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ ...seededReleased(), releasedDevices: [] }),
      );
      relayHolds.wraps = [callWrap()];
      mount();
      await settle();
      expect(presses()).toHaveLength(0);
    });
  });

  describe("what is still done for a released machine", () => {
    it("is held in the roster the carrier gets, unpaired and marked released, and drops out after 14 days", async () => {
      mount();
      await settle();
      act(() => api.unpairDevice("c1", "d1"));
      const roster = buildCarrierRoster(current().children, current().releasedDevices);
      expect(roster).toEqual([
        { machine: machinePk, childName: "Robin", deviceLabel: "Laptop", paired: false, released: true },
      ]);
      expect(buildCarrierRoster(current().children, current().releasedDevices, Date.now() + RELEASED_GRACE_MS + DAY)).toEqual([]);
    });

    it("keeps reading the released device's own relays, so the flush can be heard", async () => {
      mount();
      await settle();
      act(() => api.unpairDevice("c1", "d1"));
      relayHolds.asked = [];
      await settle(31_000);
      expect(relayHolds.asked.length).toBeGreaterThan(0);
      expect(relayHolds.asked.every((r) => r.includes(OWN_RELAY))).toBe(true);
    });
  });
});
