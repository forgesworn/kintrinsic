import { describe, expect, it } from "vitest";
import type { Device, NamedTime, Policy } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { defaultGiftDeviceIds, defaultGiftTime, giftDevices, giftOptions, timeChoices } from "./timesLever";

const LAPTOP: Device = { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) };
const PHONE: Device = { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: "b".repeat(64) };

// Monday 26 October 2026, 10:00 UTC.
const MON_10 = Date.UTC(2026, 9, 26, 10, 0, 0);
const status = (over: Partial<DeviceStatus>): DeviceStatus =>
  ({ v: 1, subject: "c", machine: "m", ts: MON_10 / 1000, dayKey: "2026-10-26", usedTodaySecs: 0, windowLeftSecs: 0, quotaLeftSecs: 0, effectiveSecs: 0, locked: false, source: "schedule", ...over }) as DeviceStatus;

const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", apps: ["x"] };

describe("which devices More time is aimed at", () => {
  it("defaults to the devices STATUS reports locked", () => {
    const st = { [LAPTOP.devicePubkey!]: status({ locked: true }), [PHONE.devicePubkey!]: status({ locked: false }) };
    expect(defaultGiftDeviceIds([LAPTOP, PHONE], st, MON_10)).toEqual(["d1"]);
  });

  it("is every device when none is locked, or nothing has reported", () => {
    const open = { [LAPTOP.devicePubkey!]: status({}), [PHONE.devicePubkey!]: status({}) };
    expect(defaultGiftDeviceIds([LAPTOP, PHONE], open, MON_10)).toEqual(["d1", "d2"]);
    expect(defaultGiftDeviceIds([LAPTOP, PHONE], {}, MON_10)).toEqual(["d1", "d2"]);
  });

  it("does not trust a stale report that says locked", () => {
    const stale = { [LAPTOP.devicePubkey!]: status({ locked: true, ts: MON_10 / 1000 - 3600 }) };
    expect(defaultGiftDeviceIds([LAPTOP, PHONE], stale, MON_10)).toEqual(["d1", "d2"]);
  });

  it("a typed gift lists only devices that can run times", () => {
    // A phone reporting an Android build below 49 cannot; a laptop at 800 can.
    const st = {
      [LAPTOP.devicePubkey!]: status({ appVersionCode: 800 }),
      [PHONE.devicePubkey!]: status({ appVersionCode: 40 }),
    };
    expect(giftDevices([LAPTOP, PHONE], st, true).map((d) => d.id)).toEqual(["d1"]);
    expect(giftDevices([LAPTOP, PHONE], st, false).map((d) => d.id)).toEqual(["d1", "d2"]);
  });

  it("never lists an unpaired device", () => {
    expect(giftDevices([LAPTOP, { ...PHONE, pairing: "pending" as Device["pairing"] }], {}, false).map((d) => d.id)).toEqual(["d1"]);
  });
});

describe("which kind of time More time opens on", () => {
  const policy: Policy = {
    id: "p",
    scope: { kind: "device" },
    times: [LEARNING],
    schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } },
    deviceOverrides: {
      d1: { schedule: { tz: "UTC", weekly: { mon: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }] } } },
    },
  };

  it("is the time the device is in right now", () => {
    expect(defaultGiftTime(policy, "d1", MON_10)).toBe("learning");
  });

  it("is the time that just ended, on that device's own schedule", () => {
    expect(defaultGiftTime(policy, "d1", Date.UTC(2026, 9, 26, 12, 0, 0))).toBe("learning");
  });

  it("is Free time on a device whose schedule has no such time, and for a child with no times", () => {
    expect(defaultGiftTime(policy, "d2", MON_10)).toBeUndefined();
    expect(defaultGiftTime({ ...policy, times: undefined }, "d1", MON_10)).toBeUndefined();
    expect(defaultGiftTime(policy, undefined, MON_10)).toBeUndefined();
  });
});

describe("the kinds on offer", () => {
  it("is Free time first, then each time", () => {
    expect(timeChoices([LEARNING])).toEqual([
      { id: undefined, label: "Free time" },
      { id: "learning", label: "Learning time" },
    ]);
    expect(timeChoices(undefined)).toEqual([{ id: undefined, label: "Free time" }]);
  });
});

describe("what is handed to the signer", () => {
  const all = ["d1", "d2"];
  it("is nothing at all for the old behaviour: Free time, every device", () => {
    expect(giftOptions({ timeId: undefined, timeLabel: undefined, chosenIds: ["d2", "d1"], allIds: all })).toBeUndefined();
  });

  it("names the kind and its label", () => {
    expect(giftOptions({ timeId: "learning", timeLabel: "Learning time", chosenIds: all, allIds: all })).toEqual({
      time: "learning",
      timeLabel: "Learning time",
    });
  });

  it("names the devices only when they are a subset", () => {
    expect(giftOptions({ timeId: undefined, timeLabel: undefined, chosenIds: ["d1"], allIds: all })).toEqual({ deviceIds: ["d1"] });
  });

  it("a typed gift to just the capable devices names them even when it is 'everyone I offered'", () => {
    expect(giftOptions({ timeId: "learning", timeLabel: "Learning time", chosenIds: ["d1"], allIds: all })).toEqual({
      time: "learning",
      timeLabel: "Learning time",
      deviceIds: ["d1"],
    });
  });
});
