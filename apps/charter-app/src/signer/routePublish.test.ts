import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NostrEvent } from "nostr-tools";
import {
  LOCAL_RECEIPT_WINDOW_MS,
  QueuedForHomeNetworkError,
  singleRecipient,
  withLocalRoute,
  type PublishMeta,
  type RoutedWrap,
} from "./routePublish";

const WARD = "a".repeat(64);
const RELAYS = ["wss://one.example", "wss://two.example"];

function wrap(over: Partial<NostrEvent> = {}): NostrEvent {
  return {
    id: "1".repeat(64),
    pubkey: "e".repeat(64),
    created_at: 1_800_000_000,
    kind: 1059,
    tags: [["p", WARD]],
    content: "ciphertext",
    sig: "f".repeat(128),
    ...over,
  };
}

interface Bridge {
  send: ReturnType<typeof vi.fn>;
  delivery: ReturnType<typeof vi.fn>;
  /** What localDelivery reports for the wrap. */
  state: { current: string | null };
}

/** Install a carrier bridge whose local route is available. */
function installBridge(sendState: string, deliveryState: string | null = null): Bridge {
  const state = { current: deliveryState };
  const send = vi.fn(() => JSON.stringify({ state: sendState }));
  const delivery = vi.fn((idsJson: string) => {
    const ids = JSON.parse(idsJson) as string[];
    if (state.current === null) return "{}";
    return JSON.stringify(
      Object.fromEntries(ids.map((id) => [id, { state: state.current, at: 1, machine: WARD }])),
    );
  });
  window.CharterCarrier = {
    provision: vi.fn(),
    isCarrier: () => true,
    localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
    localSend: send,
    localDelivery: delivery,
    localInbox: () => "[]",
    localConsume: vi.fn(),
  };
  return { send, delivery, state };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  delete window.CharterCarrier;
  vi.restoreAllMocks();
});

describe("singleRecipient", () => {
  it("accepts a 1059 with exactly one p tag", () => {
    expect(singleRecipient(wrap())).toBe(WARD);
  });
  it("rejects other kinds, no p tag, several p tags, or a malformed key", () => {
    expect(singleRecipient(wrap({ kind: 1 }))).toBeNull();
    expect(singleRecipient(wrap({ tags: [] }))).toBeNull();
    expect(singleRecipient(wrap({ tags: [["p", WARD], ["p", "b".repeat(64)]] }))).toBeNull();
    expect(singleRecipient(wrap({ tags: [["p", "nothex"]] }))).toBeNull();
  });
});

describe("withLocalRoute: no local route", () => {
  it("is exactly the relay publish when there is no bridge (a browser)", async () => {
    const relay = vi.fn(async () => {});
    const publish = withLocalRoute(relay);
    const ev = wrap();
    await publish(RELAYS, ev, { kind: "clause" });
    expect(relay).toHaveBeenCalledTimes(1);
    expect(relay).toHaveBeenCalledWith(RELAYS, ev);
  });

  it("keeps the relay error text when relays fail and there is no bridge", async () => {
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    await expect(withLocalRoute(relay)(RELAYS, wrap())).rejects.toThrow(
      "Couldn't reach any relay — check your connection and try again.",
    );
  });

  it("treats an older carrier (bridge without local methods) as no route", async () => {
    window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true };
    const relay = vi.fn(async () => {});
    await withLocalRoute(relay)(RELAYS, wrap());
    expect(relay).toHaveBeenCalledTimes(1);
  });

  it("treats a route the carrier says is unavailable as no route", async () => {
    const b = installBridge("sent", "received");
    window.CharterCarrier!.localInfo = () => JSON.stringify({ v: 1, available: false, listening: false, sessions: [] });
    const relay = vi.fn(async () => {});
    await withLocalRoute(relay)(RELAYS, wrap());
    expect(b.send).not.toHaveBeenCalled();
    expect(relay).toHaveBeenCalledTimes(1);
  });

  it("treats malformed bridge JSON as unavailable", async () => {
    installBridge("sent", "received");
    window.CharterCarrier!.localInfo = () => "not json";
    const relay = vi.fn(async () => {});
    await withLocalRoute(relay)(RELAYS, wrap());
    expect(relay).toHaveBeenCalledTimes(1);
  });

  it("does not route a wrap with several recipients, or a non-wrap", async () => {
    const b = installBridge("sent", "received");
    const relay = vi.fn(async () => {});
    const publish = withLocalRoute(relay);
    await publish(RELAYS, wrap({ tags: [["p", WARD], ["p", "b".repeat(64)]] }));
    await publish(RELAYS, wrap({ kind: 1 }));
    expect(b.send).not.toHaveBeenCalled();
    expect(relay).toHaveBeenCalledTimes(2);
  });
});

describe("withLocalRoute: with a local route", () => {
  it("starts the relay publish and the local send together", async () => {
    const b = installBridge("sent", "received");
    const order: string[] = [];
    const relay = vi.fn(() => {
      order.push("relay");
      return new Promise<void>(() => {});
    });
    b.send.mockImplementation(() => {
      order.push("local");
      return JSON.stringify({ state: "sent" });
    });
    await withLocalRoute(relay)(RELAYS, wrap());
    expect(order).toEqual(["relay", "local"]);
    expect(b.send).toHaveBeenCalledWith(WARD, JSON.stringify(wrap()));
  });

  it("resolves on the ward's receipt before a hanging relay publish returns", async () => {
    const b = installBridge("sent", "received");
    const relay = vi.fn(() => new Promise<void>(() => {})); // never settles
    const done = vi.fn();
    const p = withLocalRoute(relay)(RELAYS, wrap()).then(done);
    await vi.advanceTimersByTimeAsync(0);
    expect(done).toHaveBeenCalledTimes(1);
    await p;
    expect(b.delivery).toHaveBeenCalledWith(JSON.stringify([wrap().id]));
  });

  it("polls every 150 ms and resolves once the receipt appears", async () => {
    const b = installBridge("sent", "sent");
    const relay = vi.fn(() => new Promise<void>(() => {}));
    const done = vi.fn();
    void withLocalRoute(relay)(RELAYS, wrap()).then(done);
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(450);
    expect(done).not.toHaveBeenCalled();
    expect(b.delivery.mock.calls.length).toBeGreaterThanOrEqual(3);
    b.state.current = "received";
    await vi.advanceTimersByTimeAsync(150);
    expect(done).toHaveBeenCalledTimes(1);
  });

  it("counts any ward ACK result as receipt", async () => {
    installBridge("sent", "applied");
    const relay = vi.fn(() => new Promise<void>(() => {}));
    await expect(withLocalRoute(relay)(RELAYS, wrap())).resolves.toBeUndefined();
  });

  it("swallows a relay rejection that happens after the local route succeeded", async () => {
    installBridge("sent", "received");
    let rejectRelay!: (e: Error) => void;
    const relay = vi.fn(
      () =>
        new Promise<void>((_, reject) => {
          rejectRelay = reject;
        }),
    );
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    try {
      await withLocalRoute(relay)(RELAYS, wrap());
      rejectRelay(new Error("Couldn't reach any relay — check your connection and try again."));
      await vi.advanceTimersByTimeAsync(10);
      await Promise.resolve();
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", unhandled);
    }
  });

  it("does not hold the guardian for the full window when a relay already took the wrap", async () => {
    installBridge("sent", "sent");
    const relay = vi.fn(async () => {});
    const started = Date.now();
    await withLocalRoute(relay)(RELAYS, wrap());
    expect(Date.now() - started).toBeLessThan(LOCAL_RECEIPT_WINDOW_MS);
  });

  it("falls back to the relay result when the carrier rejects the wrap", async () => {
    const b = installBridge("rejected");
    const relay = vi.fn(async () => {});
    await expect(withLocalRoute(relay)(RELAYS, wrap())).resolves.toBeUndefined();
    expect(b.delivery).not.toHaveBeenCalled();

    const failing = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    await expect(withLocalRoute(failing)(RELAYS, wrap())).rejects.toThrow("Couldn't reach any relay");
  });

  it("falls back to relays when the carrier says unavailable or answers garbage", async () => {
    installBridge("unavailable");
    const relay = vi.fn(async () => {});
    await expect(withLocalRoute(relay)(RELAYS, wrap())).resolves.toBeUndefined();
    installBridge("sent");
    window.CharterCarrier!.localSend = () => "garbage";
    await expect(withLocalRoute(relay)(RELAYS, wrap())).resolves.toBeUndefined();
  });

  it("when sent but never confirmed and relays fail, the relay error stands", async () => {
    installBridge("sent", "sent");
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const p = withLocalRoute(relay)(RELAYS, wrap());
    const assertion = expect(p).rejects.toThrow("Couldn't reach any relay");
    await vi.advanceTimersByTimeAsync(LOCAL_RECEIPT_WINDOW_MS + 200);
    await assertion;
  });

  it("queued + relay failure throws QueuedForHomeNetworkError naming the device", async () => {
    installBridge("queued");
    const relay = vi.fn(async () => {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    });
    const err = await withLocalRoute(relay, { deviceName: () => "Robin's laptop" })(RELAYS, wrap()).catch(
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(QueuedForHomeNetworkError);
    expect((err as Error).message).toBe("Not sent yet — it'll reach Robin's laptop when it's on the home network.");
  });

  it("falls back to 'their device' when no name is known", async () => {
    installBridge("queued");
    const relay = vi.fn(async () => {
      throw new Error("relay down");
    });
    const err = await withLocalRoute(relay)(RELAYS, wrap()).catch((e: unknown) => e);
    expect((err as Error).message).toBe("Not sent yet — it'll reach their device when it's on the home network.");
  });

  it("queued + relay success is plain success (the relay copy is durable)", async () => {
    installBridge("queued");
    await expect(withLocalRoute(async () => {})(RELAYS, wrap())).resolves.toBeUndefined();
  });

  it("with zero relays, queued is 'not sent yet' and received is success", async () => {
    installBridge("queued");
    const relay = vi.fn(async () => {});
    await expect(withLocalRoute(relay)([], wrap())).rejects.toBeInstanceOf(QueuedForHomeNetworkError);
    expect(relay).not.toHaveBeenCalled();

    installBridge("sent", "received");
    await expect(withLocalRoute(relay)([], wrap())).resolves.toBeUndefined();
  });

  it("reports the wrap to the tracker with its recipient, state and meta", async () => {
    installBridge("sent", "received");
    const seen: RoutedWrap[] = [];
    const meta: PublishMeta = { kind: "grant", reqId: "r".repeat(64) };
    await withLocalRoute(async () => {}, { onRouted: (w) => seen.push(w) })(RELAYS, wrap(), meta);
    expect(seen).toEqual([{ wrapId: wrap().id, machine: WARD, state: "sent", meta }]);

    installBridge("rejected");
    seen.length = 0;
    await withLocalRoute(async () => {}, { onRouted: (w) => seen.push(w) })(RELAYS, wrap(), meta);
    expect(seen).toEqual([]); // not taken by the carrier, nothing to track
  });

  it("a throwing tracker never fails a delivery", async () => {
    installBridge("sent", "received");
    await expect(
      withLocalRoute(async () => {}, { onRouted: () => { throw new Error("boom"); } })(RELAYS, wrap()),
    ).resolves.toBeUndefined();
  });
});

describe("QueuedForHomeNetworkError", () => {
  it("names several devices once each", () => {
    const e = QueuedForHomeNetworkError.merge([
      new QueuedForHomeNetworkError(["Pixel"]),
      new QueuedForHomeNetworkError(["Laptop"]),
      new QueuedForHomeNetworkError(["Pixel"]),
    ]);
    expect(e.message).toBe("Not sent yet — it'll reach Pixel and Laptop when they're on the home network.");
  });
});
