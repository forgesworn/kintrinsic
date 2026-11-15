import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import {
  ACCEPTED_UNCONFIRMED,
  CANNOT_RESET_REMOTELY,
  devicesWithoutScreenLock,
  HELD_FOR_HOME_NETWORK,
  isValidScreenLockPin,
  NO_RESPONSE_IN_AN_HOUR,
  NO_SCREEN_LOCK_WARNING,
  screenLockReading,
  screenLockResultText,
  screenLockSupport,
  WAITING_FOR_DEVICE,
} from "./screenLock";

const KEY = "aa".repeat(32);
const ID = "0123456789abcdef0123456789abcdef";
const NOW = 1_800_000_000;

const phone = (over: Partial<Device> = {}): Device => ({
  id: "p1",
  label: "Robin’s phone",
  platform: "android",
  pairing: "paired",
  devicePubkey: KEY,
  ...over,
});
const st = (over: Partial<DeviceStatus> = {}) =>
  ({ appVersionCode: 56, screenLockSecure: true, ...over }) as DeviceStatus;

describe("screenLockSupport", () => {
  it("offers a current phone that reports screenLockSecure", () => {
    expect(screenLockSupport(phone(), { [KEY]: st() })).toBe("offer");
    expect(screenLockSupport(phone(), { [KEY]: st({ screenLockSecure: false }) })).toBe("offer");
  });
  it("hides it without screenLockSecure, with no status, on an older ward, on a laptop, or unpaired", () => {
    expect(screenLockSupport(phone(), { [KEY]: st({ screenLockSecure: undefined }) })).toBe("hidden");
    expect(screenLockSupport(phone(), {})).toBe("hidden");
    expect(screenLockSupport(phone(), { [KEY]: st({ appVersionCode: 55 }) })).toBe("hidden");
    expect(screenLockSupport(phone({ platform: "linux" }), { [KEY]: st({ appVersionCode: 9999 }) })).toBe("hidden");
    expect(screenLockSupport(phone({ pairing: "unpaired", devicePubkey: null }), { [KEY]: st() })).toBe("hidden");
  });
});

describe("devicesWithoutScreenLock", () => {
  it("names only an Android device that reports false", () => {
    const a = phone();
    const b = phone({ id: "p2", devicePubkey: "bb".repeat(32) });
    const c = phone({ id: "p3", devicePubkey: "cc".repeat(32) });
    const status = {
      [KEY]: st({ screenLockSecure: false }),
      ["bb".repeat(32)]: st({ screenLockSecure: true }),
    };
    expect(devicesWithoutScreenLock([a, b, c], status).map((d) => d.id)).toEqual(["p1"]);
  });
});

describe("isValidScreenLockPin", () => {
  it.each([["1234", true], ["1234567890123456", true], ["123", false], ["12345678901234567", false], ["12a4", false], ["", false]])(
    "%j -> %s",
    (pin, ok) => expect(isValidScreenLockPin(pin)).toBe(ok),
  );
});

describe("screenLockReading", () => {
  const sent = { id: ID, issuedAt: NOW - 60 };

  it("says Screen lock: on, or No screen lock with the open-at-any-hour sentence's flag", () => {
    expect(screenLockReading(undefined, st(), NOW, "Robin")).toMatchObject({
      state: "Screen lock: on",
      noLock: false,
    });
    expect(screenLockReading(undefined, st({ screenLockSecure: false }), NOW, "Robin")).toMatchObject({
      state: "No screen lock",
      noLock: true,
    });
    expect(NO_SCREEN_LOCK_WARNING).toBe("Anyone holding this device can use its open-at-any-hour apps.");
  });

  it("is not ready only when the ward says false; a missing report is not 'not ready'", () => {
    const notReady = screenLockReading(undefined, st({ screenLockResetReady: false }), NOW, "Robin");
    expect(notReady.ready).toBe(false);
    expect(notReady.notReadyNote).toBe("Ready after Robin next unlocks this device.");
    expect(screenLockReading(undefined, st({ screenLockResetReady: true }), NOW, "Robin").ready).toBe(true);
    expect(screenLockReading(undefined, st(), NOW, "Robin")).toMatchObject({ ready: true, notReadyNote: null });
  });

  it("says plainly when Android won't let the ward reset this phone at all", () => {
    expect(CANNOT_RESET_REMOTELY).toBe(
      "Kintrinsic can't reset this device's screen lock. It had a screen lock before Kintrinsic was set up, and Android doesn't allow it after that. Setting the device up again from a factory reset fixes this.",
    );
    // Such a phone also reports not ready; "ready after the next unlock" would be untrue.
    const r = screenLockReading(
      undefined,
      st({ screenLockResetSupported: false, screenLockResetReady: false }),
      NOW,
      "Robin",
    );
    expect(r).toMatchObject({
      state: "Screen lock: on",
      supported: false,
      ready: false,
      notReadyNote: null,
      unsupportedNote: CANNOT_RESET_REMOTELY,
    });
    // The state and the no-lock flag still read as usual.
    expect(
      screenLockReading(undefined, st({ screenLockSecure: false, screenLockResetSupported: false }), NOW, "Robin"),
    ).toMatchObject({ state: "No screen lock", noLock: true, supported: false });
    // Supported, or not reported: no note.
    for (const v of [true, undefined]) {
      expect(screenLockReading(undefined, st({ screenLockResetSupported: v }), NOW, "Robin")).toMatchObject({
        supported: true,
        unsupportedNote: null,
      });
    }
    // A reset sent before the ward said so, answered not_ready: not "try after the next unlock".
    const answered = screenLockReading(
      sent,
      st({ screenLockResetSupported: false, screenLockResets: [{ id: ID, result: "not_ready", at: NOW }] }),
      NOW,
      "Robin",
    );
    expect(answered.progress?.tone).toBe("warn");
    expect(answered.progress?.text).not.toContain("next unlocks");
    expect(answered.progress?.text).toContain("Android doesn't allow");
  });

  it("shows nothing before anything is sent", () => {
    expect(screenLockReading(undefined, st(), NOW, "Robin").progress).toBeNull();
  });

  it("waits for the device while there is no outcome for its id", () => {
    expect(screenLockReading(sent, st(), NOW, "Robin").progress).toEqual({ tone: "waiting", text: WAITING_FOR_DEVICE });
    // Another reset's outcome is not this one's.
    const other = st({ screenLockResets: [{ id: "f".repeat(32), result: "applied", at: NOW }] });
    expect(screenLockReading(sent, other, NOW, "Robin").progress?.text).toBe("Waiting for the device…");
  });

  it("expires after the hour the ward would still accept it", () => {
    const edge = { id: ID, issuedAt: NOW - 3599 };
    expect(screenLockReading(edge, st(), NOW, "Robin").progress?.text).toBe(WAITING_FOR_DEVICE);
    const late = { id: ID, issuedAt: NOW - 3600 };
    expect(screenLockReading(late, st(), NOW, "Robin").progress).toEqual({ tone: "warn", text: NO_RESPONSE_IN_AN_HOUR });
    expect(NO_RESPONSE_IN_AN_HOUR).toBe("The device didn't respond within an hour. Try again when it's online.");
  });

  it("an outcome that arrives late still wins over the expiry", () => {
    const late = { id: ID, issuedAt: NOW - 7200 };
    const status = st({ screenLockResets: [{ id: ID, result: "applied", at: NOW - 7000 }] });
    expect(screenLockReading(late, status, NOW, "Robin").progress?.tone).toBe("ok");
  });

  it("drops the sent line after a day", () => {
    expect(screenLockReading({ id: ID, issuedAt: NOW - 86_400 }, st(), NOW, "Robin").progress).toBeNull();
  });

  it("says a send only this phone holds is held for the home network, for the same hour", () => {
    const held = { id: ID, issuedAt: NOW - 60, held: true };
    expect(screenLockReading(held, st(), NOW, "Robin").progress).toEqual({
      tone: "waiting",
      text: HELD_FOR_HOME_NETWORK,
    });
    expect(HELD_FOR_HOME_NETWORK).toBe("Held until the device is on your home network.");
    expect(screenLockReading({ ...held, issuedAt: NOW - 3600 }, st(), NOW, "Robin").progress).toEqual({
      tone: "warn",
      text: NO_RESPONSE_IN_AN_HOUR,
    });
    // Once the ward answers, its answer is what shows.
    const status = st({ screenLockResets: [{ id: ID, result: "applied", at: NOW }] });
    expect(screenLockReading(held, status, NOW, "Robin").progress?.tone).toBe("ok");
  });

  it("an accepted reset unconfirmed an hour after the send says so, not 'yet'", () => {
    const status = st({ screenLockResets: [{ id: ID, result: "accepted", at: NOW - 3590 }] });
    expect(screenLockReading({ id: ID, issuedAt: NOW - 3599 }, status, NOW, "Robin").progress).toEqual({
      tone: "waiting",
      text: "The device received it but hasn't confirmed the result yet.",
    });
    expect(screenLockReading({ id: ID, issuedAt: NOW - 3600 }, status, NOW, "Robin").progress).toEqual({
      tone: "warn",
      text: ACCEPTED_UNCONFIRMED,
    });
    expect(ACCEPTED_UNCONFIRMED).toBe(
      "The device received it but didn't confirm the result. If nothing has changed, send it again.",
    );
  });
});

describe("screenLockResultText", () => {
  const at = new Date(2026, 10, 14, 19, 42).getTime() / 1000;
  const text = (r: string) => screenLockResultText(r, at, "Robin");

  it("applied: Done at 19:42", () => {
    expect(text("applied")).toEqual({ tone: "ok", text: "Done at 19:42" });
  });
  it("accepted: received, not confirmed", () => {
    expect(text("accepted")).toEqual({
      tone: "waiting",
      text: "The device received it but hasn't confirmed the result yet.",
    });
  });
  it("rejected_time and superseded use the agreed copy", () => {
    expect(text("rejected_time").text).toBe(
      "The device turned it down: its clock and yours disagree, or it arrived more than an hour late.",
    );
    expect(text("superseded").text).toBe("A newer reset replaced this one.");
  });
  it("failed, not_ready and notice_failed are plain English and name the ward", () => {
    expect(text("failed")).toEqual({ tone: "warn", text: "The device couldn't change the screen lock. Try again." });
    expect(text("not_ready").text).toContain("ready after Robin next unlocks it");
    expect(text("notice_failed").text).toContain("couldn't tell Robin");
    expect(text("notice_failed").text).toContain("left the screen lock as it was");
  });
  it("an unknown result is shown as unrecognised, never as waiting or done", () => {
    expect(text("from_the_future")).toMatchObject({ tone: "warn" });
    expect(text("from_the_future").text).toContain("doesn't understand");
  });
});
