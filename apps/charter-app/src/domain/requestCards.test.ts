import { describe, expect, it } from "vitest";
import { appOpenRequestToCard, timeExtendRequestToCard } from "./requestCards";
import type { AppOpenDeviceRequest, TimeExtendDeviceRequest } from "../wire/request";
import type { Device } from "./types";

const DEVICE: Device = {
  id: "dev1",
  label: "Sam's laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: "aa".repeat(32),
};

const REQ_ID = "ab".repeat(32);
const NONCE = "cd".repeat(32);
const SUBJECT = "ef".repeat(32);
const NOW_MS = 1_700_000_000_000;

function timeExtendReq(over: Partial<TimeExtendDeviceRequest["params"]> = {}): TimeExtendDeviceRequest {
  return {
    v: 1,
    op: "time.extend",
    reqId: REQ_ID,
    nonce: NONCE,
    subject: SUBJECT,
    machine: DEVICE.devicePubkey!,
    ts: 1_699_999_900,
    params: { minutesRequested: 20, limitHit: "budget", ...over },
  };
}

function appOpenReq(over: Partial<AppOpenDeviceRequest["params"]> = {}): AppOpenDeviceRequest {
  return {
    v: 1,
    op: "app.open",
    reqId: REQ_ID,
    nonce: NONCE,
    subject: SUBJECT,
    machine: DEVICE.devicePubkey!,
    ts: 1_699_999_900,
    params: { pkg: "com.mojang.minecraftpe", ...over },
  };
}

describe("timeExtendRequestToCard", () => {
  it("maps the wire fields onto the card, including wire correlation", () => {
    const card = timeExtendRequestToCard(timeExtendReq(), DEVICE, "child_1", "req_1", NOW_MS);
    expect(card).toMatchObject({
      id: "req_1",
      childId: "child_1",
      requester: "device",
      deviceId: "dev1",
      kind: "time.extend",
      minutesRequested: 20,
      limitHit: "budget",
      status: "pending",
      reqId: REQ_ID,
      nonce: NONCE,
      machine: DEVICE.devicePubkey,
      subject: SUBJECT,
    });
    expect(card.bucketId).toBeUndefined();
  });

  it("carries bucketId verbatim for a bucket-hit ask (named times)", () => {
    const card = timeExtendRequestToCard(
      timeExtendReq({ limitHit: "bucket", bucketId: "play" }),
      DEVICE,
      "child_1",
      "req_1",
      NOW_MS,
    );
    expect(card.limitHit).toBe("bucket");
    expect(card.bucketId).toBe("play");
  });

  it("never dates the card into the future — clamps to the wall clock", () => {
    const card = timeExtendRequestToCard(
      { ...timeExtendReq(), ts: 9_999_999_999 }, // far future device clock
      DEVICE,
      "child_1",
      "req_1",
      NOW_MS,
    );
    expect(card.createdAt).toBe(NOW_MS);
  });
});

describe("appOpenRequestToCard", () => {
  it("maps pkg -> appId and label -> appLabel, with wire correlation", () => {
    const card = appOpenRequestToCard(
      appOpenReq({ label: "Minecraft", minutesRequested: 30, reason: "almost done" }),
      DEVICE,
      "child_1",
      "req_2",
      NOW_MS,
    );
    expect(card).toMatchObject({
      id: "req_2",
      childId: "child_1",
      kind: "app.open",
      appId: "com.mojang.minecraftpe",
      appLabel: "Minecraft",
      minutesRequested: 30,
      reason: "almost done",
      status: "pending",
      reqId: REQ_ID,
      nonce: NONCE,
      machine: DEVICE.devicePubkey,
      subject: SUBJECT,
    });
  });

  it("falls back to the raw pkg as the label when the device sent none", () => {
    const card = appOpenRequestToCard(appOpenReq(), DEVICE, "child_1", "req_2", NOW_MS);
    expect(card.appLabel).toBe("com.mojang.minecraftpe");
    expect(card.title).toContain("com.mojang.minecraftpe");
  });

  // Belt and braces (F1 review): a `cmdline:` identity should never itself be
  // device-reported as an app.open pkg, but if a stale saved policy ever
  // echoed one back with no label, the card must never show the raw needle.
  it("never surfaces a raw cmdline: string as the label, even with no explicit label", () => {
    const card = appOpenRequestToCard(
      appOpenReq({ pkg: "cmdline:net.minecraft.client.main.Main" }),
      DEVICE,
      "child_1",
      "req_2",
      NOW_MS,
    );
    expect(card.appLabel).toBe("Minecraft");
    expect(card.appLabel).not.toMatch(/^cmdline:/);
  });

  // New-4 (review, 2026-08-03): the wire allows an EMPTY-STRING label
  // (distinct from an absent one), and `label ?? pkg` would treat "" as
  // present — this is the one surviving path a raw needle could reach the
  // card's label through before `identityDisplayLabel`'s own fix.
  it("never surfaces a raw cmdline: string when the wire sent an empty-string label", () => {
    const card = appOpenRequestToCard(
      appOpenReq({ pkg: "cmdline:net.minecraft.client.main.Main", label: "" }),
      DEVICE,
      "child_1",
      "req_2",
      NOW_MS,
    );
    expect(card.appLabel).toBe("Minecraft");
    expect(card.appLabel).not.toMatch(/^cmdline:/);
  });
});
