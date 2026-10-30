// An ask that names a kind of time lands on its card with that `time`; one that
// does not stays exactly as before. Through the real provider and the real
// intake (the carrier's home-network inbox), with only the relay faked.
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

function askWrap(time?: string): NostrEvent {
  const ts = Math.floor(Date.now() / 1000);
  const req: DeviceRequest = {
    v: 1,
    op: "time.extend",
    reqId: REQ_ID,
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: machinePk,
    ts,
    params: {
      minutesRequested: 30,
      reason: "homework",
      limitHit: "budget",
      ...(time !== undefined ? { time } : {}),
    },
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
  return <div data-testid="reqs">{state.requests.map((r) => `${r.reqId}:${r.time ?? "-"}`).join(",")}</div>;
}

describe("a ward's typed ask → console", () => {
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

  async function arrive(wrap: NostrEvent) {
    bridge(wrap, vi.fn());
    mount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
  }

  it("carries the asked-for time onto the card", async () => {
    await arrive(askWrap("learning"));
    expect(reqs()).toBe(`${REQ_ID}:learning`);
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;
    expect(stored.requests[0].time).toBe("learning");
  });

  it("an ask with no time has none (Free time, as before)", async () => {
    await arrive(askWrap());
    expect(reqs()).toBe(`${REQ_ID}:-`);
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY)!) as CharterState;
    expect("time" in stored.requests[0]).toBe(false);
  });

  it("an ask naming the reserved Free id is not one a real ward makes, and is dropped", async () => {
    await arrive(askWrap("free"));
    expect(reqs()).toBe("");
  });
});
