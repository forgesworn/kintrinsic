import { describe, expect, it } from "vitest";
import type { Device } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { linuxUpdateOffer, parseDebManifest, type DebManifest } from "./updateCheck";

const SHA = "3e".repeat(32);
const GITHUB =
  "https://github.com/forgesworn/kintrinsic/releases/download/linux-v0.7.13/kintrinsic_0.7.13_amd64.deb";
const BLOSSOM = `https://nostr.download/${SHA}.deb`;

const DEB: DebManifest = {
  versionName: "0.7.13",
  versionCode: 713,
  url: BLOSSOM,
  urls: [GITHUB, BLOSSOM],
  sha256: SHA,
  sizeBytes: 12_930_410,
  builtAt: "2026-10-28T00:00:00Z",
};

const LAPTOP_KEY = "bb".repeat(32);

function device(over: Partial<Device> = {}): Device {
  return {
    id: "l1",
    label: "Robin’s laptop",
    platform: "linux",
    pairing: "paired",
    devicePubkey: LAPTOP_KEY,
    ...over,
  } as Device;
}

function reporting(appVersionCode: number | undefined): Record<string, DeviceStatus> {
  if (appVersionCode === undefined) return {};
  return {
    [LAPTOP_KEY]: {
      v: 1,
      subject: "cd".repeat(32),
      machine: LAPTOP_KEY,
      ts: 1000,
      dayKey: "2026-10-28",
      usedTodaySecs: 0,
      windowLeftSecs: 0,
      quotaLeftSecs: 0,
      effectiveSecs: 1800,
      locked: false,
      source: "guardian",
      appVersionCode,
    },
  };
}

describe("linuxUpdateOffer", () => {
  it("offers Install update to a charterd at 0.7.12 or later, GitHub Release first", () => {
    const offer = linuxUpdateOffer(device(), reporting(712), DEB);
    expect(offer).toEqual({ kind: "install", deb: DEB, url: GITHUB });
  });

  it("keeps the download link for a charterd below 0.7.12", () => {
    expect(linuxUpdateOffer(device(), reporting(711), DEB)).toEqual({ kind: "download", deb: DEB });
  });

  it("keeps the download link when the feed names no sha256", () => {
    const noSha: DebManifest = { ...DEB };
    delete noSha.sha256;
    expect(linuxUpdateOffer(device(), reporting(712), noSha)?.kind).toBe("download");
  });

  it("offers nothing when up to date, silent, unpaired or a phone", () => {
    expect(linuxUpdateOffer(device(), reporting(713), DEB)).toBeNull();
    expect(linuxUpdateOffer(device(), reporting(800), DEB)).toBeNull();
    expect(linuxUpdateOffer(device(), reporting(undefined), DEB)).toBeNull();
    expect(linuxUpdateOffer(device({ pairing: "unpaired" }), reporting(712), DEB)).toBeNull();
    expect(linuxUpdateOffer(device({ platform: "android" }), reporting(712), DEB)).toBeNull();
    expect(linuxUpdateOffer(device(), reporting(712), null)).toBeNull();
  });

  it("falls back to the single url on an older feed", () => {
    const single: DebManifest = { ...DEB };
    delete single.urls;
    expect(linuxUpdateOffer(device(), reporting(712), single)).toEqual({
      kind: "install",
      deb: single,
      url: BLOSSOM,
    });
  });
});

describe("parseDebManifest sha256", () => {
  const base = { versionName: "0.7.13", versionCode: 713, sizeBytes: 1, builtAt: "x" };

  it("keeps a well-formed sha256", () => {
    expect(parseDebManifest({ ...base, sha256: SHA })?.sha256).toBe(SHA);
  });

  it("refuses a malformed one and tolerates its absence", () => {
    expect(parseDebManifest({ ...base, sha256: "XYZ" })).toBeNull();
    expect(parseDebManifest(base)?.sha256).toBeUndefined();
  });
});
