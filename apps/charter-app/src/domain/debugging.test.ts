import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import { debuggingLabel, debuggingSupport, debuggingView } from "./debugging";

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
