import { afterEach, describe, expect, it, vi } from "vitest";
import type { NostrEvent } from "nostr-tools";
import {
  localBridgeSupport,
  localConsume,
  localDelivery,
  localInbox,
  localInfo,
  localRouteAvailable,
  localRouteGap,
  localSend,
} from "./localLink";
import { localRouteGapLine, readLocalRoute } from "./localRoute";

const M = "a".repeat(64);
const WRAP = { id: "1".repeat(64), kind: 1059, tags: [["p", M]], content: "x" } as unknown as NostrEvent;

afterEach(() => {
  delete window.CharterCarrier;
});

function fullBridge(over: Partial<NonNullable<Window["CharterCarrier"]>> = {}) {
  window.CharterCarrier = {
    provision: vi.fn(),
    isCarrier: () => true,
    localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [{ machine: M, since: 5 }] }),
    localSend: () => JSON.stringify({ state: "sent" }),
    localDelivery: () => JSON.stringify({ [WRAP.id]: { state: "applied", at: 9, machine: M } }),
    localInbox: () => JSON.stringify([{ id: WRAP.id, machine: M, wrap: WRAP, receivedAt: 3 }]),
    localConsume: vi.fn(),
    ...over,
  };
}

describe("feature detection", () => {
  it("a plain browser has no bridge: everything is unavailable and nothing throws", () => {
    expect(localBridgeSupport()).toBe("none");
    expect(localInfo()).toBeNull();
    expect(localSend(M, WRAP)).toBeNull();
    expect(localDelivery([WRAP.id])).toBeNull();
    expect(localInbox()).toBeNull();
    expect(localConsume([WRAP.id])).toBe(false);
    expect(localRouteAvailable()).toBe(false);
    expect(localRouteGap()).toBe("browser");
  });

  it("an older carrier (no local methods) is detected and named as such", () => {
    window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true, roster: vi.fn() };
    expect(localBridgeSupport()).toBe("none");
    expect(localRouteGap()).toBe("old-carrier");
    expect(localInfo()).toBeNull();
    expect(localInbox()).toBeNull();
    expect(localRouteAvailable()).toBe(false);
  });

  it("a partial bridge is not treated as a working route", () => {
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
    };
    expect(localBridgeSupport()).toBe("partial");
    expect(localRouteGap()).toBe("old-carrier");
    // …and with only localInfo present, sending cannot be attempted.
    expect(localRouteAvailable()).toBe(false);
    expect(localSend(M, WRAP)).toBeNull();
    expect(localInbox()).toBeNull();
    expect(localDelivery(["x"])).toBeNull();
    expect(localConsume(["x"])).toBe(false);
  });

  it("reports a full bridge as usable", () => {
    fullBridge();
    expect(localBridgeSupport()).toBe("full");
    expect(localRouteAvailable()).toBe(true);
    expect(localRouteGap()).toBeNull();
  });

  it("calls methods on the bridge object, never detached", () => {
    const receivers: unknown[] = [];
    const bridge = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo(this: unknown) {
        receivers.push(this);
        return JSON.stringify({ v: 1, available: true, listening: true, sessions: [] });
      },
      localSend: () => "{}",
      localDelivery: () => "{}",
      localInbox: () => "[]",
      localConsume: () => {},
    };
    window.CharterCarrier = bridge;
    localInfo();
    expect(receivers).toEqual([bridge]);
    expect(receivers[0]).toBe(bridge);
  });
});

describe("parsing", () => {
  it("parses info, send, delivery and inbox", () => {
    fullBridge();
    expect(localInfo()).toEqual({ available: true, listening: true, sessions: [{ machine: M, since: 5 }] });
    expect(localSend(M, WRAP)).toBe("sent");
    expect(localDelivery([WRAP.id])).toEqual({ [WRAP.id]: { state: "applied", at: 9, machine: M } });
    expect(localInbox()).toEqual([{ id: WRAP.id, machine: M, wrap: WRAP, receivedAt: 3 }]);
  });

  it("sends the wrap as JSON and the ids as a JSON array", () => {
    const send = vi.fn(() => JSON.stringify({ state: "queued" }));
    const consume = vi.fn();
    fullBridge({ localSend: send, localConsume: consume });
    expect(localSend(M, WRAP)).toBe("queued");
    expect(send).toHaveBeenCalledWith(M, JSON.stringify(WRAP));
    expect(localConsume(["a", "b"])).toBe(true);
    expect(consume).toHaveBeenCalledWith(JSON.stringify(["a", "b"]));
    expect(localConsume([])).toBe(false);
  });

  it("treats malformed JSON as unavailable (null), never as data", () => {
    fullBridge({
      localInfo: () => "{nope",
      localSend: () => "<html>",
      localDelivery: () => "[]",
      localInbox: () => "{}",
    });
    expect(localInfo()).toBeNull();
    expect(localSend(M, WRAP)).toBeNull();
    expect(localDelivery(["x"])).toBeNull();
    expect(localInbox()).toBeNull();
    expect(localRouteAvailable()).toBe(false);
  });

  it("treats a throwing bridge as unavailable", () => {
    const boom = () => {
      throw new Error("bridge down");
    };
    fullBridge({ localInfo: boom, localSend: boom, localDelivery: boom, localInbox: boom, localConsume: boom });
    expect(localInfo()).toBeNull();
    expect(localSend(M, WRAP)).toBeNull();
    expect(localDelivery(["x"])).toBeNull();
    expect(localInbox()).toBeNull();
    expect(localConsume(["x"])).toBe(false);
  });

  it("drops unknown send states, unknown delivery states and malformed rows", () => {
    fullBridge({
      localSend: () => JSON.stringify({ state: "teleported" }),
      localDelivery: () =>
        JSON.stringify({ a: { state: "applied", at: 1, machine: M }, b: { state: "weird" }, c: 7 }),
      localInbox: () => JSON.stringify([{ id: "ok", machine: M, wrap: WRAP, receivedAt: 1 }, { id: 5 }, null, { id: "nowrap" }]),
      localInfo: () =>
        JSON.stringify({ available: true, listening: false, sessions: [{ machine: "short" }, { machine: M, since: 1 }, null] }),
    });
    expect(localSend(M, WRAP)).toBeNull();
    expect(Object.keys(localDelivery(["a", "b", "c"])!)).toEqual(["a"]);
    expect(localInbox()!.map((e) => e.id)).toEqual(["ok"]);
    expect(localInfo()!.sessions).toEqual([{ machine: M, since: 1 }]);
  });

  it("says the route is not running when the carrier reports unavailable", () => {
    fullBridge({ localInfo: () => JSON.stringify({ v: 1, available: false, listening: false, sessions: [] }) });
    expect(localRouteGap()).toBe("not-running");
  });
});

describe("what the screens say", () => {
  it("names the browser, the older carrier and the idle listener in plain words", () => {
    expect(localRouteGapLine("browser")).toBe(
      "Home-network delivery needs the Kintrinsic Android app. Here, changes travel through relays.",
    );
    expect(localRouteGapLine("old-carrier")).toBe("Update the Kintrinsic app to deliver over your home network.");
    expect(localRouteGapLine("not-running")).toMatch(/isn't running right now/);
    expect(localRouteGapLine(null)).toBeNull();
  });

  it("exposes live sessions even when the listener is otherwise idle", () => {
    fullBridge({
      localInfo: () => JSON.stringify({ v: 1, available: false, listening: false, sessions: [{ machine: M, since: 1 }] }),
    });
    const view = readLocalRoute();
    expect(view.gap).toBe("not-running");
    expect(view.sessions.has(M)).toBe(true);
  });

  it("has no sessions in a browser or older carrier", () => {
    expect(readLocalRoute().sessions.size).toBe(0);
    window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true };
    expect(readLocalRoute()).toMatchObject({ gap: "old-carrier" });
  });
});
