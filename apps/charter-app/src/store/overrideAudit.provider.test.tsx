// The ward's emergency unlock reaching the guardian's Activity history through
// both inbound routes (relay poll, home-network inbox). One press is ONE line,
// at the moment it was pressed, however many wraps carry it and however often
// the app is reloaded; and the carrier may forget a wrap only once that line is
// in storage.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { generateSecretKey, getPublicKey, nip19, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import { overrideTags, overrideWrap } from "../test/overrideWrap";

// The relay pool is the only network this exercises: replace it so the poll
// returns exactly the wraps the test says the relay holds.
// By default it ignores the filter (every wrap it holds is returned each read).
// `honourSince` makes it behave like a real relay for the late-wrap tests, and
// `sinceLog` records the `since` each read asked for.
const relayHolds: { wraps: NostrEvent[]; honourSince: boolean; sinceLog: number[] } = {
  wraps: [],
  honourSince: false,
  sinceLog: [],
};
vi.mock("nostr-tools", async (importOriginal) => {
  const real = await importOriginal<typeof import("nostr-tools")>();
  class FakePool {
    async querySync(_relays: string[], filter: { since?: number }): Promise<NostrEvent[]> {
      if (filter.since !== undefined) relayHolds.sinceLog.push(filter.since);
      if (!relayHolds.honourSince) return relayHolds.wraps;
      return relayHolds.wraps.filter((w) => w.created_at >= (filter.since ?? 0));
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

function wrap(over: { at?: number; builtAt?: number } = {}): NostrEvent {
  const at = over.at ?? pressedAt;
  return overrideWrap({
    guardianPk,
    machineSk,
    tags: overrideTags([["at", String(at)]]),
    createdAt: over.builtAt ?? Math.floor(Date.now() / 1000),
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
  const lines = api.state.activity.filter((a) => a.outcome === "override");
  return <div data-testid="lines">{JSON.stringify(lines)}</div>;
}

describe("emergency unlock → Activity", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded()));
    relayHolds.wraps = [];
    relayHolds.honourSince = false;
    relayHolds.sinceLog = [];
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

  const lines = () =>
    JSON.parse(container.querySelector('[data-testid="lines"]')!.textContent!) as Array<{
      childId: string;
      ts: number;
      summary: string;
      auditKey: string;
    }>;

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

  it("lands one override line for the right child, at the moment it was pressed", async () => {
    relayHolds.wraps = [wrap()];
    mount();
    await settle();

    const got = lines();
    expect(got).toHaveLength(1);
    expect(got[0].childId).toBe("c1");
    expect(got[0].ts).toBe(pressedAt * 1000);
    expect(got[0].summary).toBe("Opened their phone on Laptop for 5 minutes");
    expect(got[0].auditKey).toBe(`${machinePk}:${pressedAt}`);
  });

  it("the same wrap from the relay and the home network is one line", async () => {
    const w = wrap();
    relayHolds.wraps = [w];
    bridge([w]);
    mount();
    await settle();
    await settle(6_000); // more drains and a relay re-poll
    expect(lines()).toHaveLength(1);
  });

  it("a wrap rebuilt after a day offline (new id, same press time) is still one line", async () => {
    // Pressed two hours ago; the wrap was first built an hour ago, then again now.
    pressedAt = Math.floor(Date.now() / 1000) - 7200;
    const first = wrap({ builtAt: Math.floor(Date.now() / 1000) - 3600 });
    const rebuilt = wrap({ builtAt: Math.floor(Date.now() / 1000) });
    expect(rebuilt.id).not.toBe(first.id);
    // Both in the same tick, then the second alone on a later poll.
    relayHolds.wraps = [first, rebuilt];
    mount();
    await settle();
    expect(lines()).toHaveLength(1);
    relayHolds.wraps = [rebuilt];
    await settle(31_000);
    expect(lines()).toHaveLength(1);
  });

  it("two different presses are two lines", async () => {
    relayHolds.wraps = [wrap(), wrap({ at: pressedAt + 60 })];
    mount();
    await settle();
    expect(lines().map((l) => l.ts).sort()).toEqual([pressedAt * 1000, (pressedAt + 60) * 1000]);
  });

  it("a provider remounted from persisted state adds nothing for a wrap it already recorded", async () => {
    relayHolds.wraps = [wrap()];
    mount();
    await settle();
    expect(lines()).toHaveLength(1);
    act(() => root.unmount());
    container.remove();

    // A fresh session: no in-memory handled sets, the relay still holds the day.
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).activity).toHaveLength(1);
    mount();
    await settle();
    await settle(31_000);
    expect(lines()).toHaveLength(1);
  });

  it("an unknown machine adds nothing, and the carrier is not told to forget the wrap", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded({ devicePubkey: getPublicKey(generateSecretKey()) })));
    const w = wrap();
    relayHolds.wraps = [w];
    const consume = vi.fn();
    bridge([w], consume);
    mount();
    await settle();
    await settle(6_000);
    expect(lines()).toHaveLength(0);
    expect(consume).not.toHaveBeenCalled();
  });

  it("a device paired later gets its earlier override on the next pass", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded({ pairing: "pairing" })));
    const w = wrap();
    relayHolds.wraps = [w];
    const consume = vi.fn();
    bridge([w], consume);
    mount();
    await settle();
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

  it("tells the carrier to forget the wrap only after the line is in storage", async () => {
    const w = wrap();
    const onDisk: string[] = [];
    const consume = vi.fn((ids: string[]) => {
      onDisk.push(localStorage.getItem(STORAGE_KEY) ?? "");
      expect(ids).toEqual([w.id]);
    });
    bridge([w], consume);
    mount();
    await settle();
    expect(consume).toHaveBeenCalled();
    for (const stored of onDisk) expect(stored).toContain(`${machinePk}:${pressedAt}`);
  });

  it("never dates a line in the future, even if the ward's clock ran ahead", async () => {
    const ahead = Math.floor(Date.now() / 1000) + 3600;
    relayHolds.wraps = [wrap({ at: ahead, builtAt: ahead })];
    mount();
    await settle();
    const got = lines();
    expect(got).toHaveLength(1);
    expect(got[0].ts).toBeLessThanOrEqual(Date.now());
  });

  describe("a wrap published late (built at the press, reaching a relay much later)", () => {
    // The ward builds the wrap at its first poll after the press, so its
    // created_at is old by the time an offline phone gets it to a relay. A
    // running app's narrow cursor window has long passed that time.
    const lateWrap = () => {
      const now = Math.floor(Date.now() / 1000);
      pressedAt = now - 7200;
      return wrap({ at: pressedAt, builtAt: now - 3600 });
    };

    it("is missed by narrow reads and found by the periodic sweep", async () => {
      relayHolds.honourSince = true;
      mount();
      await settle(); // the first read (a sweep) finds nothing
      relayHolds.wraps = [lateWrap()];
      await settle(31_000); // narrow ticks
      await settle(31_000);
      expect(lines()).toHaveLength(0);

      await settle(15 * 60_000);
      expect(lines()).toHaveLength(1);
      expect(lines()[0].ts).toBe(pressedAt * 1000);

      // The reads in between followed the cursor (each later than the last); the
      // sweep then reached back a full day, well before the read that preceded it.
      const reads = relayHolds.sinceLog;
      const dropped = reads.findIndex((since, i) => i > 1 && since < reads[i - 1] - 3600);
      expect(dropped).toBeGreaterThan(1);
      expect(reads[1]).toBeGreaterThan(reads[0]);
    });

    it("is found when the app comes back to the foreground, not on a narrow tick", async () => {
      relayHolds.honourSince = true;
      mount();
      await settle();
      relayHolds.wraps = [lateWrap()];
      await settle(31_000);
      expect(lines()).toHaveLength(0);

      await act(async () => {
        document.dispatchEvent(new Event("visibilitychange"));
        await vi.advanceTimersByTimeAsync(50);
      });
      expect(lines()).toHaveLength(1);
    });

    it("is found by a pull-to-refresh", async () => {
      relayHolds.honourSince = true;
      mount();
      await settle();
      relayHolds.wraps = [lateWrap()];
      await settle(31_000);
      expect(lines()).toHaveLength(0);

      await act(async () => {
        void api.refreshNow();
        await vi.advanceTimersByTimeAsync(50);
      });
      expect(lines()).toHaveLength(1);
    });
  });
});
