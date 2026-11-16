// The screen-lock reset, end to end through the real provider and the real
// local signer, with only the relay pool faked. The point is what is NOT kept:
// the PIN never reaches the state, the persisted store or the Activity log, and
// no wrap is recorded; only the reset's id and time are.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { generateSecretKey, getPublicKey, nip19, nip44, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";

const relay = vi.hoisted(() => ({ events: [] as unknown[], accept: true }));
vi.mock("nostr-tools", async (importOriginal) => {
  const real = await importOriginal<typeof import("nostr-tools")>();
  class FakePool {
    async querySync(): Promise<NostrEvent[]> {
      return [];
    }
    publish(_relays: string[], event: NostrEvent): Promise<string>[] {
      relay.events.push(event);
      return [Promise.resolve(relay.accept ? "ok" : "connection failure: relay down")];
    }
    close(): void {}
  }
  return { ...real, SimplePool: FakePool };
});

import { CharterProvider, STORAGE_KEY, useCharter, type CharterContextValue } from "./store";

const guardianSk = generateSecretKey();
const DEVICE_SK = generateSecretKey();
const PHONE = getPublicKey(DEVICE_SK);
const SECRET = "246813";

describe("resetDeviceScreenLock (provider)", () => {
  let container: HTMLDivElement;
  let root: Root;
  let api: CharterContextValue;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    relay.events = [];
    relay.accept = true;
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [
          {
            id: "c1",
            name: "Robin",
            color: "#3B6FB2",
            dependantPubkey: null,
            devices: [{ id: "d2", label: "Robin's phone", platform: "android", pairing: "paired", devicePubkey: PHONE }],
            policies: [],
          },
        ],
        requests: [],
        activity: [],
        signer: { connected: false, kind: "local", autoSign: true },
      }),
    );
    function Harness() {
      api = useCharter();
      return null;
    }
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Harness />
        </CharterProvider>,
      );
    });
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    vi.useRealTimers();
  });

  const settle = async () => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
  };

  const reset = async (digits: string): Promise<boolean> => {
    await settle(); // the persisted local signer reconnects
    let ok = false;
    await act(async () => {
      ok = await api.resetDeviceScreenLock("c1", "d2", digits);
    });
    await settle();
    return ok;
  };

  /** Open the wrap, then the sealed PIN, with the device key, as the ward does. */
  const opened = (wrap: NostrEvent) => {
    const seal = JSON.parse(nip44.decrypt(wrap.content, nip44.getConversationKey(DEVICE_SK, wrap.pubkey)));
    const inner = JSON.parse(nip44.decrypt(seal.content, nip44.getConversationKey(DEVICE_SK, seal.pubkey)));
    const payload = JSON.parse(inner.content);
    const pin = JSON.parse(nip44.decrypt(payload.newPinSealed, nip44.getConversationKey(DEVICE_SK, payload.epk)));
    return { inner, payload, pin };
  };

  it("sends one wrap to the device, carrying the PIN sealed, and records only the id and time", async () => {
    expect(await reset(SECRET)).toBe(true);
    expect(relay.events).toHaveLength(1);
    const { inner, payload, pin } = opened(relay.events[0] as NostrEvent);
    expect(inner.kind).toBe(31122);
    expect(payload).not.toHaveProperty("newPin");
    expect(pin).toBe(SECRET);

    const record = api.state.children[0].devices[0].screenLockReset;
    expect(record).toEqual({ id: payload.id, issuedAt: payload.issuedAt });
  });

  it("keeps the PIN out of the state, the persisted store and the Activity log; keeps no wraps", async () => {
    await reset(SECRET);
    expect(JSON.stringify(api.state)).not.toContain(SECRET);
    // The state WAS written (it carries the reset's id), and the PIN is not in it.
    const id = api.state.children[0].devices[0].screenLockReset!.id;
    expect(localStorage.getItem(STORAGE_KEY)).toContain(id);
    expect(localStorage.getItem(STORAGE_KEY)).not.toContain(SECRET);
    for (let i = 0; i < localStorage.length; i++) {
      expect(localStorage.getItem(localStorage.key(i)!)).not.toContain(SECRET);
    }
    const line = api.state.activity.find((a) => a.summary.includes("screen lock"));
    expect(line?.summary).toBe("You reset the screen lock on Robin's phone");
    expect(line?.wraps).toBeUndefined();
    expect(api.state.localDeliveries ?? []).toEqual([]);
  });

  it("says 'removed' for a removal, which seals an empty PIN", async () => {
    await reset("");
    expect(opened(relay.events[0] as NostrEvent).pin).toBe("");
    expect(api.state.activity.some((a) => a.summary === "You removed the screen lock on Robin's phone")).toBe(true);
  });

  it("records nothing, and does not leak the PIN, when no relay takes it", async () => {
    relay.accept = false;
    await settle();
    let failure: unknown = null;
    await act(async () => {
      await api.resetDeviceScreenLock("c1", "d2", SECRET).catch((e: unknown) => {
        failure = e;
      });
    });
    expect((failure as Error).message).toMatch(/Couldn't reach any relay/);
    expect((failure as Error).message).not.toContain(SECRET);
    expect(api.state.children[0].devices[0].screenLockReset).toBeUndefined();
    expect(JSON.stringify(api.state)).not.toContain(SECRET);
  });

  it("says so, and sends nothing, when the signer isn't connected", async () => {
    await settle();
    await act(async () => {
      await api.disconnectSigner();
    });
    let failure: unknown = null;
    await act(async () => {
      await api.resetDeviceScreenLock("c1", "d2", SECRET).catch((e: unknown) => {
        failure = e;
      });
    });
    expect((failure as Error | null)?.message).toBe(
      "Not sent: your signer isn't connected. Reconnect it and try again.",
    );
    expect(relay.events).toHaveLength(0);
    expect(api.state.children[0].devices[0].screenLockReset).toBeUndefined();
  });

  it("does nothing for an unknown device", async () => {
    await settle();
    let ok = true;
    await act(async () => {
      ok = await api.resetDeviceScreenLock("c1", "nope", SECRET);
    });
    expect(ok).toBe(false);
    expect(relay.events).toHaveLength(0);
  });
});
