import { describe, expect, it } from "vitest";
import {
  PAIRING_LINK_EXTRA,
  PROVISIONING_KEYS as K,
  QrPayloadError,
  WARD_ADMIN_COMPONENT,
  buildQrPayload,
  downloadLocation,
  MIN_QR_WARD_VERSION_CODE,
  packageChecksum,
  qrPayloadText,
} from "./qrPayload";

const CERT = "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc";
const APK = "cb04d9bc7a3ec77100d144cf22fd82b80a252dca8409b5fff905ca87aae0aac1";
const BLOSSOM = "https://nostr.download/cb04d9bc7a3ec77100d144cf22fd82b80a252dca8409b5fff905ca87aae0aac1.apk";
const GITHUB = "https://github.com/forgesworn/kintrinsic/releases/download/ward-v0.6.16/kintrinsic-ward-0.6.16.apk";
const FEED = {
  versionName: "0.6.17",
  versionCode: 48,
  url: "https://example.org/ward.apk",
  urls: [GITHUB, BLOSSOM],
  apkSha256: APK,
  certSha256: CERT,
  sizeBytes: 29702085,
  builtAt: "2026-10-28T12:01:02Z",
};
const LINK = `bunker://${"ab".repeat(32)}?relay=wss://relay.damus.io&kind=charter&token=${"0f".repeat(16)}`;

function problem(fn: () => unknown): string | undefined {
  try {
    fn();
  } catch (e) {
    if (e instanceof QrPayloadError) return e.problem;
    throw e;
  }
  return undefined;
}

describe("packageChecksum", () => {
  it("re-encodes the hex digest as URL-safe base64 without padding", () => {
    expect(packageChecksum(CERT)).toBe("Sng6PiwIeQa_KdTVACtUZwX6UPnYNE7F2tCI5HQM_Nw");
    const got = packageChecksum(APK);
    expect(got).toBe(Buffer.from(APK, "hex").toString("base64url"));
    expect(got).not.toMatch(/[+/=]/);
    expect(got).toHaveLength(43);
  });

  it("refuses a missing or malformed checksum", () => {
    expect(problem(() => packageChecksum(undefined))).toBe("no-checksum");
    expect(problem(() => packageChecksum(""))).toBe("no-checksum");
    expect(problem(() => packageChecksum(APK.slice(2)))).toBe("no-checksum");
    expect(problem(() => packageChecksum(`${APK.slice(1)}z`))).toBe("no-checksum");
  });
});

describe("downloadLocation", () => {
  it("prefers the Blossom entry in urls[] over url and the other mirrors", () => {
    expect(downloadLocation(FEED)).toBe(BLOSSOM);
  });

  it("falls back to url when no urls[] entry is on the Blossom host", () => {
    expect(downloadLocation({ ...FEED, urls: [GITHUB] })).toBe("https://example.org/ward.apk");
    expect(downloadLocation({ ...FEED, urls: undefined })).toBe("https://example.org/ward.apk");
  });

  it("passes over a non-https Blossom entry to url", () => {
    expect(downloadLocation({ ...FEED, urls: ["http://nostr.download/x.apk"] })).toBe("https://example.org/ward.apk");
  });

  it("refuses anything that is not https", () => {
    expect(problem(() => downloadLocation({ url: "http://nostr.download/x.apk" }))).toBe("no-download");
    expect(problem(() => downloadLocation({ urls: ["http://nostr.download/x.apk"] }))).toBe("no-download");
    expect(problem(() => downloadLocation({ url: "/charter-latest.apk" }))).toBe("no-download");
    expect(problem(() => downloadLocation({}))).toBe("no-download");
  });
});

describe("buildQrPayload", () => {
  it("builds the standard Device Owner extras from the feed and the link", () => {
    const p = buildQrPayload(FEED, LINK);
    expect(p).toEqual({
      "android.app.extra.PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME": WARD_ADMIN_COMPONENT,
      "android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION": BLOSSOM,
      "android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_CHECKSUM": Buffer.from(APK, "hex").toString("base64url"),
      "android.app.extra.PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED": true,
      "android.app.extra.PROVISIONING_SKIP_ENCRYPTION": false,
      "android.app.extra.PROVISIONING_ADMIN_EXTRAS_BUNDLE": { [PAIRING_LINK_EXTRA]: LINK },
    });
    expect(WARD_ADMIN_COMPONENT).toBe("org.forgesworn.charter/.admin.CharterDeviceAdminReceiver");
    expect(PAIRING_LINK_EXTRA).toBe("org.forgesworn.charter.PAIRING_LINK");
  });

  it("never carries a signature checksum, from either certificate", () => {
    const text = qrPayloadText(FEED, LINK);
    expect(text).not.toContain("SIGNATURE_CHECKSUM");
    expect(text).not.toContain(Buffer.from(CERT, "hex").toString("base64url"));
  });

  it("carries no Wi-Fi keys unless an SSID is given", () => {
    for (const wifi of [undefined, { ssid: "" }, { ssid: "", password: "secret" }]) {
      const keys = Object.keys(buildQrPayload(FEED, LINK, wifi));
      expect(keys.some((k) => k.includes("WIFI"))).toBe(false);
    }
  });

  it("adds the Wi-Fi keys when given, WPA by default with a password", () => {
    const p = buildQrPayload(FEED, LINK, { ssid: " Home ", password: "hunter22" });
    // Exactly as typed: spaces are legal in an SSID.
    expect(p[K.wifiSsid]).toBe('" Home "');
    expect(p[K.wifiPassword]).toBe("hunter22");
    expect(p[K.wifiSecurity]).toBe("WPA");
  });

  it("an open network carries no password and security NONE", () => {
    const p = buildQrPayload(FEED, LINK, { ssid: "Cafe" });
    expect(p[K.wifiSecurity]).toBe("NONE");
    expect(K.wifiPassword in p).toBe(false);
    expect(buildQrPayload(FEED, LINK, { ssid: "Old", password: "k", security: "WEP" })[K.wifiSecurity]).toBe("WEP");
  });

  it("refuses a feed with no APK checksum, even when everything else is there", () => {
    const { apkSha256: _drop, ...noSum } = FEED;
    void _drop;
    expect(problem(() => buildQrPayload(noSum, LINK))).toBe("no-checksum");
    expect(problem(() => buildQrPayload({ ...FEED, apkSha256: "abc" }, LINK))).toBe("no-checksum");
    expect(problem(() => buildQrPayload(null, LINK))).toBe("no-checksum");
  });

  it("refuses a ward release older than the provisioning activities", () => {
    expect(MIN_QR_WARD_VERSION_CODE).toBe(48);
    expect(problem(() => buildQrPayload({ ...FEED, versionCode: 47 }, LINK))).toBe("old-release");
    expect(problem(() => buildQrPayload({ ...FEED, versionCode: undefined }, LINK))).toBe("old-release");
    expect(problem(() => buildQrPayload({ ...FEED, versionCode: "48" }, LINK))).toBe("old-release");
    expect(problem(() => buildQrPayload({ ...FEED, versionCode: 49 }, LINK))).toBeUndefined();
  });

  it("refuses a feed whose only download is not https", () => {
    expect(problem(() => buildQrPayload({ ...FEED, url: "http://x/y.apk", urls: [] }, LINK))).toBe("no-download");
  });

  it("refuses a missing pairing link", () => {
    expect(problem(() => buildQrPayload(FEED, ""))).toBe("no-link");
    expect(problem(() => buildQrPayload(FEED, "https://charter.mysignet.app/pair#x"))).toBe("no-link");
  });

  it("encodes as JSON that round-trips", () => {
    expect(JSON.parse(qrPayloadText(FEED, LINK))).toEqual(buildQrPayload(FEED, LINK));
  });
});
