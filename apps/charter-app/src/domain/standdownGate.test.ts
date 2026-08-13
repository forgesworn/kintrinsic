import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import { standingStandDown } from "./standdownGate";

function device(over: Partial<Device>): Device {
  return {
    id: "d1",
    label: "Phone",
    platform: "android",
    pairing: "paired",
    devicePubkey: "ab".repeat(32),
    ...over,
  } as Device;
}

function status(over: Partial<DeviceStatus>): DeviceStatus {
  return {
    v: 1,
    subject: "cd".repeat(32),
    machine: "ab".repeat(32),
    ts: 1000,
    dayKey: "2026-07-02",
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 1800,
    locked: false,
    source: "guardian",
    ...over,
  };
}

const NOW_FRESH_MS = 1000 * 1000; // now/1000 == ts == fresh

describe("standingStandDown", () => {
  const phone = device({ id: "p1", platform: "android", devicePubkey: "aa".repeat(32) });
  const laptop = device({ id: "l1", platform: "linux", devicePubkey: "bb".repeat(32) });

  it("stands when any device freshly reports the stand-down lock", () => {
    expect(
      standingStandDown([laptop], {
        [laptop.devicePubkey as string]: status({ locked: true, lockReason: "standdown" }),
      }, NOW_FRESH_MS),
    ).toBe(true);
  });

  // A phone that went dark while stood-down must not pin "waiting on you"
  // forever — including long after the stand-down lapsed at her midnight.
  // Everywhere else the app holds the same freshness line.
  it("a stale report is not evidence the stand-down still stands", () => {
    const staleNow = (1000 + 24 * 3600) * 1000;
    expect(
      standingStandDown([phone], {
        [phone.devicePubkey as string]: status({ locked: true, lockReason: "standdown" }),
      }, staleNow),
    ).toBe(false);
  });

  it("does not stand on an ordinary lock", () => {
    expect(
      standingStandDown([phone], {
        [phone.devicePubkey as string]: status({ locked: true, lockReason: "schedule" }),
      }, NOW_FRESH_MS),
    ).toBe(false);
  });
});
