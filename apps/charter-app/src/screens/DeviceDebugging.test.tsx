import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Child, Device } from "../domain/types";
import DeviceDebugging from "./DeviceDebugging";
import { DEBUGGING_HINT, DEBUGGING_WARNING } from "../domain/debugging";

const store = vi.hoisted(() => ({
  setDeviceDebugging: vi.fn(),
  deviceStatus: {} as Record<string, unknown>,
}));
vi.mock("../store/store", () => ({ useCharter: () => store }));

const PK = "a".repeat(64);
const phone: Device = { id: "d", label: "Robin’s phone", platform: "android", pairing: "paired", devicePubkey: PK };
const child = (d: Device): Child => ({ id: "c", name: "Robin", color: "#abc", dependantPubkey: null, devices: [d], policies: [] });
const NOW = 1_800_000_000;

describe("USB debugging control", () => {
  let container: HTMLDivElement;
  let root: Root;
  function mount(device: Device) {
    act(() => {
      root = createRoot(container);
      root.render(<DeviceDebugging child={child(device)} device={device} />);
    });
  }
  const buttons = () => [...container.querySelectorAll("button")];
  async function click(label: string) {
    const b = buttons().find((x) => x.textContent === label || x.getAttribute("aria-label") === label);
    expect(b, label).toBeDefined();
    await act(async () => { b!.click(); });
  }
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(NOW * 1000);
    store.deviceStatus = {};
    store.setDeviceDebugging.mockResolvedValue(true);
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => { act(() => root.unmount()); container.remove(); vi.useRealTimers(); });

  it("shows Off with no record, and nothing for a laptop", () => {
    mount(phone);
    expect(container.textContent).toContain("USB debugging");
    expect(container.textContent).toContain("Off");
    act(() => root.unmount());
    mount({ ...phone, platform: "linux" });
    expect(container.textContent).toBe("");
  });

  it("shows 'On until you turn it off' and 'On until <time>', and Off once expired", () => {
    mount({ ...phone, debugging: { allow: true, issuedAt: NOW } });
    expect(container.textContent).toContain("On until you turn it off");
    act(() => root.unmount());
    const until = NOW + 3600;
    mount({ ...phone, debugging: { allow: true, until, issuedAt: NOW } });
    expect(container.textContent).toMatch(/On until \d\d:\d\d/);
    act(() => root.unmount());
    mount({ ...phone, debugging: { allow: true, until: NOW - 1, issuedAt: NOW - 3600 } });
    expect(container.textContent).not.toContain("On until");
    expect(buttons().map((b) => b.textContent)).toContain("Turn on");
  });

  it("says a too-old ward needs updating, with no toggle", () => {
    store.deviceStatus = { [PK]: { appVersionCode: 40 } };
    mount(phone);
    expect(container.textContent).toContain("needs the latest Kintrinsic");
    expect(buttons()).toHaveLength(0);
  });

  it("turning on shows the plain warning, sends the chosen window, then the Developer options hint", async () => {
    mount(phone);
    await click("Turn on USB debugging on Robin’s phone");
    expect(container.textContent).toContain(DEBUGGING_WARNING);
    expect(store.setDeviceDebugging).not.toHaveBeenCalled(); // not until confirmed
    await click("2 hours");
    await click("Turn on USB debugging");
    expect(store.setDeviceDebugging).toHaveBeenCalledWith("c", "d", { allow: true, until: NOW + 7200 });
    expect(container.textContent).toContain(DEBUGGING_HINT);
  });

  it("'Until I turn it off' sends no expiry; keeping it off sends nothing", async () => {
    mount(phone);
    await click("Turn on USB debugging on Robin’s phone");
    await click("Until I turn it off");
    await click("Turn on USB debugging");
    expect(store.setDeviceDebugging).toHaveBeenCalledWith("c", "d", { allow: true });
    act(() => root.unmount());
    store.setDeviceDebugging.mockClear();
    mount(phone);
    await click("Turn on USB debugging on Robin’s phone");
    await click("Keep it off");
    expect(store.setDeviceDebugging).not.toHaveBeenCalled();
  });

  it("turning off is one tap", async () => {
    mount({ ...phone, debugging: { allow: true, issuedAt: NOW } });
    await click("Turn off USB debugging on Robin’s phone");
    expect(store.setDeviceDebugging).toHaveBeenCalledWith("c", "d", { allow: false });
  });

  it("a send held for the home network is waiting, never success", async () => {
    store.setDeviceDebugging.mockResolvedValue({ outcome: "queued", message: "Not sent yet — it'll reach Robin’s phone when it's on the home network." });
    mount(phone);
    await click("Turn on USB debugging on Robin’s phone");
    await click("Turn on USB debugging");
    expect(container.textContent).toContain("Waiting for the home network");
    expect(container.textContent).not.toContain(DEBUGGING_HINT);
  });
});
