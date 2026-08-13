import { describe, it, expect, vi, afterEach } from "vitest";
import { webUsbSupported, connectPhone, ProvisionError } from "./webusb";

describe("webusb support detection", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reports unsupported when navigator.usb is absent", () => {
    vi.stubGlobal("navigator", {});
    expect(webUsbSupported()).toBe(false);
  });

  it("reports supported when navigator.usb exists", () => {
    vi.stubGlobal("navigator", { usb: {} });
    expect(webUsbSupported()).toBe(true);
  });

  it("connectPhone throws a typed unsupported error off-Chromium", async () => {
    vi.stubGlobal("navigator", {});
    await expect(connectPhone()).rejects.toMatchObject({ code: "unsupported" });
    await expect(connectPhone()).rejects.toBeInstanceOf(ProvisionError);
  });
});
