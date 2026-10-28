import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import type { Child } from "../domain/types";
import SetupPhoneQr from "./SetupPhoneQr";

const PK = "ab".repeat(32);
const TOKEN = "0f".repeat(16);
const CERT = "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc";
const APK = "cb04d9bc7a3ec77100d144cf22fd82b80a252dca8409b5fff905ca87aae0aac1";
const BLOSSOM = "https://nostr.download/cb04.apk";
const FEED = {
  versionName: "0.6.17",
  versionCode: 48,
  url: BLOSSOM,
  urls: ["https://github.com/forgesworn/kintrinsic/releases/download/x.apk", BLOSSOM],
  apkSha256: APK,
  certSha256: CERT,
};

const store = vi.hoisted(() => ({
  beginPhonePairing: vi.fn(),
  cancelPhonePairing: vi.fn(),
}));
vi.mock("../store/store", () => ({ useCharter: () => store }));
vi.mock("../signer/guardianKey", () => ({ guardianPubkeyHex: () => PK }));
vi.mock("../domain/pairingSas", () => ({ pairingSas: async () => "123 456" }));
// jsdom has no canvas; the payload rides on the <img> as a data attribute.
vi.mock("qrcode", () => ({ default: { toDataURL: async () => "data:image/png;base64,AAAA" } }));

const child: Child = { id: "c1", name: "Robin", color: "#abc", dependantPubkey: null, devices: [], policies: [] };

describe("QR setup screen", () => {
  let container: HTMLDivElement;
  let root: Root;
  let onUseCable: Mock<() => void>;
  let onBack: Mock<() => void>;
  let fetchMock: Mock;

  function respond(body: unknown, ok = true) {
    fetchMock.mockResolvedValue({ ok, status: ok ? 200 : 404, json: async () => body });
  }
  async function mount() {
    await act(async () => {
      root = createRoot(container);
      root.render(<SetupPhoneQr child={child} onUseCable={onUseCable} onBack={onBack} />);
    });
    // Let the feed fetch, the SAS and the QR render settle.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
  const img = () => container.querySelector("img");
  const payload = () => JSON.parse(img()!.getAttribute("data-provisioning-payload")!) as Record<string, unknown>;
  const button = (label: string) =>
    [...container.querySelectorAll("button")].find((b) => b.textContent?.includes(label));
  async function type(id: string, value: string) {
    const el = container.querySelector<HTMLInputElement>(`#${id}`)!;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    await act(async () => {
      setter.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }

  beforeEach(() => {
    vi.clearAllMocks();
    store.beginPhonePairing.mockReturnValue(TOKEN);
    onUseCable = vi.fn<() => void>();
    onBack = vi.fn<() => void>();
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  it("shows the steps, the code and the pairing check for a good feed", async () => {
    respond(FEED);
    await mount();
    expect(fetchMock).toHaveBeenCalledWith("/charter-apk.json", { cache: "no-store" });
    expect(store.beginPhonePairing).toHaveBeenCalledWith("c1");
    expect(container.textContent).toContain("tap the same blank spot six times");
    expect(container.textContent).toContain("123 456");
    const p = payload();
    expect(p["android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION"]).toBe(BLOSSOM);
    expect(p["android.app.extra.PROVISIONING_DEVICE_ADMIN_PACKAGE_CHECKSUM"]).toBe(
      Buffer.from(APK, "hex").toString("base64url"),
    );
    expect("android.app.extra.PROVISIONING_DEVICE_ADMIN_SIGNATURE_CHECKSUM" in p).toBe(false);
    const link = (p["android.app.extra.PROVISIONING_ADMIN_EXTRAS_BUNDLE"] as Record<string, string>)[
      "org.forgesworn.charter.PAIRING_LINK"
    ];
    expect(link.startsWith(`bunker://${PK}?`)).toBe(true);
    expect(link.endsWith(`&token=${TOKEN}`)).toBe(true);
    expect(Object.keys(p).some((k) => k.includes("WIFI"))).toBe(false);
  });

  it("adds Wi-Fi only once the guardian types it in", async () => {
    respond(FEED);
    await mount();
    expect(container.querySelector("#qr-wifi-ssid")).toBeNull();
    await act(async () => button("Add Wi-Fi details")!.click());
    expect(container.textContent).toContain("Only needed if the phone can't find your Wi-Fi itself");
    await type("qr-wifi-ssid", "Home");
    let p = payload();
    expect(p["android.app.extra.PROVISIONING_WIFI_SSID"]).toBe('"Home"');
    expect(p["android.app.extra.PROVISIONING_WIFI_SECURITY_TYPE"]).toBe("NONE");
    await type("qr-wifi-password", "hunter22");
    p = payload();
    expect(p["android.app.extra.PROVISIONING_WIFI_PASSWORD"]).toBe("hunter22");
    expect(p["android.app.extra.PROVISIONING_WIFI_SECURITY_TYPE"]).toBe("WPA");
  });

  it("never shows a code for a feed with no APK checksum, and offers the cable", async () => {
    const { apkSha256: _c, ...noSum } = FEED;
    void _c;
    respond(noSum);
    await mount();
    expect(img()).toBeNull();
    expect(container.textContent).toContain("no setup code to show");
    await act(async () => button("Set up with a cable instead")!.click());
    expect(onUseCable).toHaveBeenCalledTimes(1);
  });

  it("asks for the cable while the released ward predates QR setup", async () => {
    respond({ ...FEED, versionCode: 47 });
    await mount();
    expect(img()).toBeNull();
    expect(container.textContent).toContain("QR setup needs the next ward release; use the cable setup for now");
    await act(async () => button("Set up with a cable instead")!.click());
    expect(onUseCable).toHaveBeenCalledTimes(1);
  });

  it("says so when the feed cannot be fetched", async () => {
    fetchMock.mockRejectedValue(new Error("offline"));
    await mount();
    expect(img()).toBeNull();
    expect(container.textContent).toContain("no setup code to show");
    respond(FEED);
    await act(async () => button("Try again")!.click());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(img()).not.toBeNull();
  });

  it("gives the pairing token back when it closes", async () => {
    respond(FEED);
    await mount();
    expect(store.cancelPhonePairing).not.toHaveBeenCalled();
    act(() => root.unmount());
    expect(store.cancelPhonePairing).toHaveBeenCalled();
    // afterEach unmounts again; a fresh root keeps that harmless.
    root = createRoot(container);
  });
});
