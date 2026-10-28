import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import { debuggingLabel, debuggingReading, debuggingSupport, debuggingView } from "./debugging";

// Local time, so the clock the label prints is the clock the test builds.
const at = (h: number, m = 0, dayOffset = 0) =>
  Math.floor(new Date(2026, 9, 27 + dayOffset, h, m, 0).getTime() / 1000);

describe("debuggingLabel", () => {
  const now = at(12);
  it("is Off when nothing was ever sent, or it was turned off", () => {
    expect(debuggingLabel(undefined, now)).toBe("Off");
    expect(debuggingLabel({ allow: false, issuedAt: now - 5 }, now)).toBe("Off");
  });
  it("names the end time of a timed allowance", () => {
    expect(debuggingLabel({ allow: true, until: at(18), issuedAt: now }, now)).toBe("On until 18:00");
    expect(debuggingLabel({ allow: true, until: at(21, 5), issuedAt: now }, now)).toBe("On until 21:05");
  });
  it("adds the weekday when it ends on another day", () => {
    expect(debuggingLabel({ allow: true, until: at(1, 0, 1), issuedAt: now }, now)).toBe("On until Wed 01:00");
  });
  it("says so when there is no end", () => {
    expect(debuggingLabel({ allow: true, issuedAt: now }, now)).toBe("On until you turn it off");
  });
  it("reads an expired allowance as Off, ending AT the instant", () => {
    const sent = { allow: true, until: at(18), issuedAt: now };
    expect(debuggingLabel(sent, at(18) - 1)).toBe("On until 18:00");
    expect(debuggingLabel(sent, at(18))).toBe("Off");
    expect(debuggingView(sent, at(19))).toEqual({ state: "off" });
  });
});

describe("debuggingReading", () => {
  const now = at(12);
  const on = { allow: true, until: at(18), issuedAt: now };
  const onOpenEnded = { allow: true, issuedAt: now };
  const off = { allow: false, issuedAt: now };
  const rep = (debuggingOpen: boolean | undefined, appVersionCode: number | undefined = 47) => ({ debuggingOpen, appVersionCode });

  describe("the device reports debuggingOpen: that is the truth", () => {
    it("open and sent on reads On, with the window", () => {
      expect(debuggingReading(on, rep(true), now)).toEqual({ label: "On until 18:00", offerOff: true });
      expect(debuggingReading(onOpenEnded, rep(true), now).label).toBe("On until you turn it off");
    });
    it("open with nothing on record reads plain On", () => {
      expect(debuggingReading(undefined, rep(true), now)).toEqual({ label: "On", offerOff: true });
    });
    it("closed and never sent, or sent off, reads Off", () => {
      expect(debuggingReading(undefined, rep(false), now)).toEqual({ label: "Off", offerOff: false });
      expect(debuggingReading(off, rep(false), now)).toEqual({ label: "Off", offerOff: false });
    });
    it("closed although an on was sent: waiting for the device, or for the home network if held", () => {
      expect(debuggingReading(on, rep(false), now)).toEqual({ label: "Turned on — waiting for the device", offerOff: true });
      expect(debuggingReading({ ...on, held: true }, rep(false), now).label).toBe("Turned on — waiting for the home network");
    });
    it("closed although the on has run out reads Off, never On", () => {
      expect(debuggingReading(on, rep(false), at(19)).label).toBe("Off");
    });
    it("open although off was sent (or the window ran out): turning off, waiting", () => {
      expect(debuggingReading(off, rep(true), now)).toEqual({ label: "Turning off — waiting for the device", offerOff: true });
      expect(debuggingReading({ ...off, held: true }, rep(true), now).label).toBe("Turning off — waiting for the home network");
      expect(debuggingReading(on, rep(true), at(19)).label).toBe("Turning off — waiting for the device");
    });
    it("a held flag is ignored once the report agrees with the intention", () => {
      expect(debuggingReading({ ...on, held: true }, rep(true), now).label).toBe("On until 18:00");
      expect(debuggingReading({ ...off, held: true }, rep(false), now).label).toBe("Off");
    });
  });

  describe("an older ward that reports a version but no debuggingOpen: the sent record, as before", () => {
    it("labels from what was sent", () => {
      expect(debuggingReading(on, rep(undefined), now)).toEqual({ label: "On until 18:00", offerOff: true });
      expect(debuggingReading(undefined, rep(undefined), now)).toEqual({ label: "Off", offerOff: false });
      expect(debuggingReading(on, rep(undefined), at(19)).label).toBe("Off");
    });
    it("says a held send is waiting for the home network", () => {
      expect(debuggingReading({ ...on, held: true }, rep(undefined), now).note).toBe("Waiting for the home network.");
      expect(debuggingReading(on, rep(undefined), now).note).toBeUndefined();
    });
  });

  describe("a ward that has reported nothing: never claim On as fact", () => {
    it("an on that was sent is waiting", () => {
      expect(debuggingReading(on, undefined, now)).toEqual({ label: "Turned on — waiting for the device", offerOff: true });
      expect(debuggingReading(on, { debuggingOpen: undefined, appVersionCode: undefined }, now).label).toBe("Turned on — waiting for the device");
      expect(debuggingReading({ ...onOpenEnded, held: true }, undefined, now).label).toBe("Turned on — waiting for the home network");
    });
    it("nothing sent, or off sent, still reads Off (the default-blocked posture)", () => {
      expect(debuggingReading(undefined, undefined, now).label).toBe("Off");
      expect(debuggingReading(off, undefined, now).label).toBe("Off");
    });
  });
});

describe("debuggingSupport", () => {
  const dev = (over: Partial<Device>): Device =>
    ({ id: "d", label: "Phone", platform: "android", pairing: "paired", devicePubkey: "aa".repeat(32), ...over }) as Device;
  const reports = (appVersionCode: number) =>
    ({ [("aa").repeat(32)]: { appVersionCode } as DeviceStatus });

  it("offers an Android ward that has not reported (silence is not incapacity)", () => {
    expect(debuggingSupport(dev({}), {})).toBe("offer");
  });
  it("offers versionCode 47 and up, and flags 46 and below as too old", () => {
    expect(debuggingSupport(dev({}), reports(47))).toBe("offer");
    expect(debuggingSupport(dev({}), reports(46))).toBe("tooOld");
  });
  it("shows nothing for a Linux ward, however current", () => {
    expect(debuggingSupport(dev({ platform: "linux" }), reports(99999))).toBe("none");
  });
  it("shows nothing for a device that is not paired", () => {
    expect(debuggingSupport(dev({ pairing: "unpaired" }), {})).toBe("none");
    expect(debuggingSupport(dev({ devicePubkey: null }), {})).toBe("none");
  });
});
