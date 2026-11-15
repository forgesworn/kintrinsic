// The ward's emergency-call button reaching the guardian's Activity history
// through both inbound routes (relay poll, home-network inbox). One press is ONE
// line, at the moment it was pressed, and it never swallows (or is swallowed by)
// an emergency unlock pressed in the same second.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { generateSecretKey, getPublicKey, nip19, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import { emergencyCallTags, overrideTags, overrideWrap } from "../test/overrideWrap";

const relayHolds: { wraps: NostrEvent[] } = { wraps: [] };
vi.mock("nostr-tools", async (importOriginal) => {
  const real = await importOriginal<typeof import("nostr-tools")>();
  class FakePool {
    async querySync(): Promise<NostrEvent[]> {
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

const guardianSk = generateSecretKey();
const guardianPk = getPublicKey(guardianSk);
const machineSk = generateSecretKey();
const machinePk = getPublicKey(machineSk);

let pressedAt: number; // unix seconds, set per test
let api: ReturnType<typeof useCharter>;

function callWrap(over: { at?: number; builtAt?: number } = {}): NostrEvent {
  return overrideWrap({
    guardianPk,
    machineSk,
    tags: emergencyCallTags([["at", String(over.at ?? pressedAt)]]),
    createdAt: over.builtAt ?? Math.floor(Date.now() / 1000),
  });
}

function unlockWrap(at: number): NostrEvent {
  return overrideWrap({
    guardianPk,
    machineSk,
    tags: overrideTags([["at", String(at)]]),
    createdAt: Math.floor(Date.now() / 1000),
  });
}

function seeded(over: { devicePubkey?: string; pairing?: "paired" | "pairing" } = {}): CharterState {
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
            pairing: over.pairing ?? "paired",
            devicePubkey: over.devicePubkey ?? machinePk,
          },
        ],
      },
    ],
    requests: [],
    activity: [],
    signer: { connected: false, kind: "local", autoSign: true },
  };
}

function Probe() {
  api = useCharter();
  return <div data-testid="lines">{JSON.stringify(api.state.activity)}</div>;
}

describe("emergency call button \u2192 Activity", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded()));
    relayHolds.wraps = [];
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

  type Line = { childId: string; ts: number; outcome: string; summary: string; auditKey: string };
  const all = () =>
    JSON.parse(container.querySelector('[data-testid="lines"]')!.textContent!) as Line[];
  const lines = () => all().filter((l) => l.outcome === "emergency_call");

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
        JSON.stringify(wraps.map((w) => ({ id: w.id, machine: machinePk, wrap: w, receivedAt: 1 }))),
      localConsume: (json: string) => onConsume(JSON.parse(json) as string[]),
    };
  }

  it("lands one emergency-call line for the right child, at the press time, with honest wording", async () => {
    relayHolds.wraps = [callWrap()];
    mount();
    await settle();
    const got = lines();
    expect(got).toHaveLength(1);
    expect(got[0].childId).toBe("c1");
    expect(got[0].ts).toBe(pressedAt * 1000);
    expect(got[0].summary).toBe(
      "Used the emergency call button on Laptop. The emergency dialler opened; Kintrinsic can't see whether a call was made.",
    );
    expect(got[0].auditKey).toBe(`emergencycall:${machinePk}:${pressedAt}`);
  });

  it("the same wrap from the relay and the home network is one line", async () => {
    const w = callWrap();
    relayHolds.wraps = [w];
    bridge([w]);
    mount();
    await settle();
    await settle(6_000);
    expect(lines()).toHaveLength(1);
  });

  it("a wrap rebuilt after a day offline (new id, same press time) is still one line", async () => {
    pressedAt = Math.floor(Date.now() / 1000) - 7200;
    const first = callWrap({ builtAt: Math.floor(Date.now() / 1000) - 3600 });
    const rebuilt = callWrap({ builtAt: Math.floor(Date.now() / 1000) });
    expect(rebuilt.id).not.toBe(first.id);
    relayHolds.wraps = [first, rebuilt];
    mount();
    await settle();
    expect(lines()).toHaveLength(1);
    relayHolds.wraps = [rebuilt];
    await settle(31_000);
    expect(lines()).toHaveLength(1);
  });

  it("two presses are two lines", async () => {
    relayHolds.wraps = [callWrap(), callWrap({ at: pressedAt + 60 })];
    mount();
    await settle();
    expect(lines().map((l) => l.ts).sort()).toEqual([pressedAt * 1000, (pressedAt + 60) * 1000]);
  });

  it("a break-glass press and an emergency call in the same second are two lines", async () => {
    relayHolds.wraps = [unlockWrap(pressedAt), callWrap()];
    mount();
    await settle();
    await settle(31_000);
    const got = all();
    expect(got).toHaveLength(2);
    expect(got.map((l) => l.outcome).sort()).toEqual(["emergency_call", "override"]);
    expect(got.every((l) => l.ts === pressedAt * 1000)).toBe(true);
    expect(new Set(got.map((l) => l.auditKey)).size).toBe(2);
  });

  it("a remount from persisted state adds nothing for a press it already recorded", async () => {
    relayHolds.wraps = [callWrap()];
    mount();
    await settle();
    expect(lines()).toHaveLength(1);
    act(() => root.unmount());
    container.remove();
    mount();
    await settle();
    await settle(31_000);
    expect(lines()).toHaveLength(1);
  });

  it("an unknown machine is unlanded (the carrier keeps the wrap), then lands once the phone is paired", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded({ pairing: "pairing" })));
    const w = callWrap();
    relayHolds.wraps = [w];
    const consume = vi.fn();
    bridge([w], consume);
    mount();
    await settle();
    await settle(6_000);
    expect(lines()).toHaveLength(0);
    expect(consume).not.toHaveBeenCalled();

    act(() => {
      api.reconnectDevice("c1", "d1");
    });
    await settle(6_000);
    await settle(31_000);
    expect(lines()).toHaveLength(1);
    expect(lines()[0].ts).toBe(pressedAt * 1000);
    expect(consume).toHaveBeenCalled();
  });

  it("an unrelated machine adds nothing", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded({ devicePubkey: getPublicKey(generateSecretKey()) })));
    relayHolds.wraps = [callWrap()];
    mount();
    await settle();
    expect(lines()).toHaveLength(0);
  });

  it("tells the carrier to forget the wrap only after the line is in storage", async () => {
    const w = callWrap();
    const onDisk: string[] = [];
    const consume = vi.fn((ids: string[]) => {
      onDisk.push(localStorage.getItem(STORAGE_KEY) ?? "");
      expect(ids).toEqual([w.id]);
    });
    bridge([w], consume);
    mount();
    await settle();
    expect(consume).toHaveBeenCalled();
    for (const stored of onDisk) expect(stored).toContain(`emergencycall:${machinePk}:${pressedAt}`);
  });

  it("never dates a line in the future, even if the ward's clock ran ahead", async () => {
    const ahead = Math.floor(Date.now() / 1000) + 3600;
    relayHolds.wraps = [callWrap({ at: ahead, builtAt: ahead })];
    mount();
    await settle();
    const got = lines();
    expect(got).toHaveLength(1);
    expect(got[0].ts).toBeLessThanOrEqual(Date.now());
  });
});
