// The provider's two inbound routes meeting: the relay poll and the carrier's
// home-network inbox. One ask arriving by both must land ONCE, and the carrier
// may only be told to forget a wrap after the ask is safely in storage.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";
import { CHARTER_DEVICE_REQUEST, type DeviceRequest } from "../wire/request";

// The relay pool is the only network this exercises: replace it so the poll
// returns exactly the wraps the test says the relay holds.
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
const REQ_ID = "9f".repeat(32);

function askWrap(): NostrEvent {
  const ts = Math.floor(Date.now() / 1000);
  const req: DeviceRequest = {
    v: 1,
    op: "time.extend",
    reqId: REQ_ID,
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

function seeded(): CharterState {
  return {
    children: [
      {
        id: "c1",
        name: "Robin",
        color: "#3B6FB2",
        dependantPubkey: null,
        policies: [{ id: "p1", scope: { kind: "device" } }],
        devices: [{ id: "d1", label: "Laptop", platform: "linux", pairing: "paired", devicePubkey: machinePk }],
      },
    ],
    requests: [],
    activity: [],
    signer: { connected: false, kind: "local", autoSign: true },
  };
}

function Probe() {
  const { state } = useCharter();
  return <div data-testid="reqs">{state.requests.map((r) => r.reqId).join(",")}</div>;
}

describe("home-network inbox → console", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    localStorage.setItem(STORAGE_KEY, JSON.stringify(seeded()));
    relayHolds.wraps = [];
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

  const reqs = () => container.querySelector('[data-testid="reqs"]')!.textContent;

  function bridge(wrap: NostrEvent, onConsume: (ids: string[]) => void) {
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
      localSend: () => JSON.stringify({ state: "unavailable" }),
      localDelivery: () => "{}",
      localInbox: () => JSON.stringify([{ id: wrap.id, machine: machinePk, wrap, receivedAt: 1 }]),
      localConsume: (json: string) => onConsume(JSON.parse(json) as string[]),
    };
  }

  it("lands an ask handed up by the carrier, and consumes it only after the state is on disk", async () => {
    const wrap = askWrap();
    const seenOnDisk: string[] = [];
    const consume = vi.fn((ids: string[]) => {
      seenOnDisk.push(localStorage.getItem(STORAGE_KEY) ?? "");
      expect(ids).toEqual([wrap.id]);
    });
    bridge(wrap, consume);
    mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });

    expect(reqs()).toBe(REQ_ID);
    expect(consume).toHaveBeenCalled();
    // At the moment the carrier was told to forget it, the ask was already stored.
    expect(seenOnDisk[0]).toContain(REQ_ID);
  });

  it("one ask arriving by relay AND the home network is one inbox entry", async () => {
    const wrap = askWrap();
    relayHolds.wraps = [wrap];
    const consume = vi.fn();
    bridge(wrap, consume);
    mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    // Several more drains (the carrier in this fake never forgets) and a relay re-poll.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6_000);
    });

    expect(reqs()).toBe(REQ_ID); // exactly one, not "id,id"
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;
    expect(stored.requests.filter((r) => r.reqId === REQ_ID)).toHaveLength(1);
    // The duplicate copy is still released so it does not loop.
    expect(consume).toHaveBeenCalled();
  });

  it("does nothing, and does not throw, in a plain browser with no bridge", async () => {
    mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(reqs()).toBe("");
  });

  it("does nothing on a carrier that predates the inbox", async () => {
    window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true };
    mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(reqs()).toBe("");
  });
});
