import { describe, expect, it } from "vitest";
import {
  PAIRING_LINK_EXTRA,
  PROVISIONING_KEYS as K,
  QrPayloadError,
  WARD_ADMIN_COMPONENT,
  buildQrPayload,
  downloadLocation,
  qrPayloadText,
  signatureChecksum,
} from "./qrPayload";

const CERT = "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc";
const BLOSSOM = "https://nostr.download/cb04d9bc7a3ec77100d144cf22fd82b80a252dca8409b5fff905ca87aae0aac1.apk";
const GITHUB = "https://github.com/forgesworn/kintrinsic/releases/download/ward-v0.6.16/kintrinsic-ward-0.6.16.apk";
const FEED = {
  versionName: "0.6.16",
  versionCode: 47,
  url: "https://example.org/ward.apk",
  urls: [GITHUB, BLOSSOM],
  apkSha256: "cb04d9bc7a3ec77100d144cf22fd82b80a252dca8409b5fff905ca87aae0aac1",
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

describe("signatureChecksum", () => {
  it("re-encodes the hex digest as URL-safe base64 without padding", () => {
    const got = signatureChecksum(CERT);
    expect(got).toBe(Buffer.from(CERT, "hex").toString("base64url"));
    expect(got).toBe("Sng6PiwIeQa_KdTVACtUZwX6UPnYNE7F2tCI5HQM_Nw");
    expect(got).not.toMatch(/[+/=]/);
    expect(got).toHaveLength(43);
  });

  it("refuses a missing or malformed certificate", () => {
    expect(problem(() => signatureChecksum(undefined))).toBe("no-cert");
    expect(problem(() => signatureChecksum(""))).toBe("no-cert");
    expect(problem(() => signatureChecksum(CERT.slice(2)))).toBe("no-cert");
    expect(problem(() => signatureChecksum(`${CERT.slice(1)}z`))).toBe("no-cert");
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
      "android.app.extra.PROVISIONING_DEVICE_ADMIN_SIGNATURE_CHECKSUM": "Sng6PiwIeQa_KdTVACtUZwX6UPnYNE7F2tCI5HQM_Nw",
      "android.app.extra.PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED": true,
      "android.app.extra.PROVISIONING_SKIP_ENCRYPTION": false,
      "android.app.extra.PROVISIONING_ADMIN_EXTRAS_BUNDLE": { [PAIRING_LINK_EXTRA]: LINK },
    });
    expect(WARD_ADMIN_COMPONENT).toBe("org.forgesworn.charter/.admin.CharterDeviceAdminReceiver");
    expect(PAIRING_LINK_EXTRA).toBe("org.forgesworn.charter.PAIRING_LINK");
  });

  it("carries no Wi-Fi keys unless an SSID is given", () => {
    for (const wifi of [undefined, { ssid: "" }, { ssid: "   ", password: "secret" }]) {
      const keys = Object.keys(buildQrPayload(FEED, LINK, wifi));
      expect(keys.some((k) => k.includes("WIFI"))).toBe(false);
    }
  });

  it("adds the Wi-Fi keys when given, WPA by default with a password", () => {
    const p = buildQrPayload(FEED, LINK, { ssid: " Home ", password: "hunter22" });
    expect(p[K.wifiSsid]).toBe('"Home"');
    expect(p[K.wifiPassword]).toBe("hunter22");
    expect(p[K.wifiSecurity]).toBe("WPA");
  });

  it("an open network carries no password and security NONE", () => {
    const p = buildQrPayload(FEED, LINK, { ssid: "Cafe" });
    expect(p[K.wifiSecurity]).toBe("NONE");
    expect(K.wifiPassword in p).toBe(false);
    expect(buildQrPayload(FEED, LINK, { ssid: "Old", password: "k", security: "WEP" })[K.wifiSecurity]).toBe("WEP");
  });

  it("refuses a feed with no certificate, even when everything else is there", () => {
    const { certSha256: _drop, ...noCert } = FEED;
    void _drop;
    expect(problem(() => buildQrPayload(noCert, LINK))).toBe("no-cert");
    expect(problem(() => buildQrPayload(null, LINK))).toBe("no-cert");
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
