import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import type { Child, Device } from "../domain/types";
import { INSTALL_WINDOW_MINUTES } from "./installWindow";

// The store is the seam: these tests are about what the pages show and which
// store call each control makes, not about delivery.
const store = vi.hoisted(() => ({ current: null as unknown }));
vi.mock("../store/store", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../store/store")>()),
  useCharter: () => store.current,
}));

// Home stands in for "nothing to show here": it has its own tests.
vi.mock("./Home", () => ({ default: () => <div data-testid="home">HOME</div> }));

import {
  AddDevicePage,
  ChildDeviceSettings,
  ChildDevices,
  ChildManage,
  DevicePage,
} from "./ChildAdmin";
import { deviceProblemLine } from "./deviceProblem";

const KEY = "a".repeat(64);
const KEY2 = "b".repeat(64);

const laptop = (over: Partial<Device> = {}): Device => ({
  id: "d1",
  label: "Sam's laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: KEY,
  ...over,
});
const phone = (over: Partial<Device> = {}): Device => ({
  id: "d2",
  label: "Sam's phone",
  platform: "android",
  pairing: "paired",
  devicePubkey: KEY2,
  ...over,
});
const sam = (devices: Device[]): Child => ({
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices,
  policies: [],
});

function makeStore(children: Child[], extra: Record<string, unknown> = {}) {
  return {
    state: { children, requests: [], activity: [], signer: { connected: true, kind: "local", autoSign: false } },
    deviceStatus: {},
    updateManifest: null,
    debManifest: null,
    phonePairing: null,
    unpairDevice: vi.fn(),
    removeDevice: vi.fn(),
    reconnectDevice: vi.fn(),
    sendLinuxUpdate: vi.fn(async () => true),
    sendCharterUpdate: vi.fn(async () => true),
    openMaintenanceWindow: vi.fn(async () => true),
    editChild: vi.fn(),
    removeChild: vi.fn(),
    createRelayCode: vi.fn(),
    recordDeviceRelays: vi.fn(),
    setDeviceDebugging: vi.fn(),
    ...extra,
  };
}

describe("child administration", () => {
  let container: HTMLDivElement;
  let root: Root;

  const render = (ui: JSX.Element, s: ReturnType<typeof makeStore>) => {
    store.current = s;
    act(() => root.render(ui));
  };
  const labels = () =>
    [...container.querySelectorAll("button")].map((b) => b.getAttribute("aria-label") ?? b.textContent?.trim());
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find(
      (el) => el.textContent?.trim() === label || el.getAttribute("aria-label") === label,
    );
    if (!b) throw new Error(`no button "${label}" in: ${labels().join(" | ")}`);
    return b as HTMLButtonElement;
  };
  const click = (label: string) => act(() => button(label).click());

  beforeEach(() => {
    window.location.hash = "";
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    window.location.hash = "";
  });

  describe("the device page", () => {
    it("a paired device offers Unlock, Disconnect and Relays, and not Reconnect or Remove", () => {
      const c = sam([laptop()]);
      render(<DevicePage childId="c1" deviceId="d1" />, makeStore([c]));
      expect(button("Unlock Sam's laptop with a code")).toBeTruthy();
      expect(button("Disconnect Sam's laptop")).toBeTruthy();
      expect(button("Relays")).toBeTruthy();
      expect(labels()).not.toContain("Reconnect Sam's laptop");
      expect(labels()).not.toContain("Remove Sam's laptop");
      // The unlock code says which case it covers.
      expect(container.textContent).toContain("locked and can’t get online");
    });

    it("an unpaired device offers Reconnect and Remove, and not Unlock, Disconnect or Relays", () => {
      const c = sam([laptop({ pairing: "unpaired" })]);
      const s = makeStore([c]);
      render(<DevicePage childId="c1" deviceId="d1" />, s);
      expect(button("Reconnect Sam's laptop")).toBeTruthy();
      expect(button("Remove Sam's laptop")).toBeTruthy();
      expect(labels()).not.toContain("Unlock Sam's laptop with a code");
      expect(labels()).not.toContain("Disconnect Sam's laptop");
      expect(labels()).not.toContain("Relays");

      click("Reconnect Sam's laptop");
      expect(s.reconnectDevice).toHaveBeenCalledWith("c1", "d1");
      expect(window.location.hash).toBe("");
    });

    it("Disconnect asks first, then goes back to the child's settings", () => {
      const s = makeStore([sam([laptop()])]);
      render(<DevicePage childId="c1" deviceId="d1" />, s);
      click("Disconnect Sam's laptop");
      expect(s.unpairDevice).not.toHaveBeenCalled();
      expect(container.textContent).toContain("Kintrinsic stops managing it");
      click("Keep");
      expect(button("Disconnect Sam's laptop")).toBeTruthy();
      click("Disconnect Sam's laptop");
      click("Disconnect Sam's laptop");
      expect(s.unpairDevice).toHaveBeenCalledWith("c1", "d1");
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("leaving after Disconnect or Remove replaces the page, so Back doesn't return to it", () => {
      const before = window.history.length;
      const s = makeStore([sam([laptop()])]);
      render(<DevicePage childId="c1" deviceId="d1" />, s);
      click("Disconnect Sam's laptop");
      click("Disconnect Sam's laptop");
      expect(window.location.hash).toBe("#/ward/c1/settings");
      expect(window.history.length).toBe(before);

      window.location.replace("#/ward/c1/device/d1");
      const gone = makeStore([sam([laptop({ pairing: "unpaired" })])]);
      render(<DevicePage childId="c1" deviceId="d1" />, gone);
      click("Remove Sam's laptop");
      expect(window.location.hash).toBe("#/ward/c1/settings");
      expect(window.history.length).toBe(before);
    });

    it("hands the exit to the shell when it provides one, instead of replacing the address", () => {
      const done = vi.fn();
      const s = makeStore([sam([laptop()])]);
      window.location.hash = "/ward/c1/device/d1";
      render(<DevicePage childId="c1" deviceId="d1" onDone={done} />, s);
      click("Disconnect Sam's laptop");
      click("Disconnect Sam's laptop");
      expect(done).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe("#/ward/c1/device/d1");
    });

    it("a laptop that took the update sent to it reads 'Updated ✓' once it has caught up", async () => {
      const deb = {
        versionName: "0.7.13",
        versionCode: 713,
        url: "https://example.org/kintrinsic.deb",
        sha256: "c".repeat(64),
        sizeBytes: 1000,
        builtAt: "2026-10-29T00:00:00Z",
      };
      const reporting = (code: number) => ({
        [KEY]: { appVersionCode: code, appVersionName: String(code) },
      });
      const behind = makeStore([sam([laptop()])], { debManifest: deb, deviceStatus: reporting(712) });
      render(<DevicePage childId="c1" deviceId="d1" />, behind);
      expect(container.textContent).not.toContain("Updated ✓");
      await act(async () => button("Install Kintrinsic 0.7.13 on Sam's laptop").click());
      expect(behind.sendLinuxUpdate).toHaveBeenCalledWith("c1", "d1");
      expect(container.textContent).toContain("Update sent.");
      expect(container.textContent).not.toContain("Updated ✓");

      const caughtUp = makeStore([sam([laptop()])], { debManifest: deb, deviceStatus: reporting(713) });
      render(<DevicePage childId="c1" deviceId="d1" />, caughtUp);
      expect(container.textContent).toContain("Updated ✓");
      expect(container.textContent).not.toContain("Update sent.");
    });

    it("an update only held for the home network never reads 'Updated ✓'", async () => {
      const deb = {
        versionName: "0.7.13",
        versionCode: 713,
        url: "https://example.org/kintrinsic.deb",
        sha256: "c".repeat(64),
        sizeBytes: 1000,
        builtAt: "2026-10-29T00:00:00Z",
      };
      const reporting = (code: number) => ({ [KEY]: { appVersionCode: code, appVersionName: String(code) } });
      const behind = makeStore([sam([laptop()])], {
        debManifest: deb,
        deviceStatus: reporting(712),
        sendLinuxUpdate: vi.fn(async () => ({ message: "Held for the home network." })),
      });
      render(<DevicePage childId="c1" deviceId="d1" />, behind);
      await act(async () => button("Install Kintrinsic 0.7.13 on Sam's laptop").click());
      const caughtUp = makeStore([sam([laptop()])], { debManifest: deb, deviceStatus: reporting(713) });
      render(<DevicePage childId="c1" deviceId="d1" />, caughtUp);
      expect(container.textContent).not.toContain("Updated ✓");
    });

    it("Remove goes back to the child's settings", () => {
      const s = makeStore([sam([laptop({ pairing: "unpaired" })])]);
      render(<DevicePage childId="c1" deviceId="d1" />, s);
      click("Remove Sam's laptop");
      expect(s.removeDevice).toHaveBeenCalledWith("c1", "d1");
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("shows the status notes before the actions", () => {
      const s = makeStore([sam([phone()])], {
        deviceStatus: {
          [KEY2]: { appVersionName: "0.6.17", updateHealth: { attempts: 4, sinceUnix: 1_700_000_000, lastError: "disk full" } },
        },
      });
      render(<DevicePage childId="c1" deviceId="d2" />, s);
      const text = container.textContent ?? "";
      expect(text).toContain("Kintrinsic 0.6.17");
      expect(text).toContain("Update isn’t completing — disk full");
      expect(text.indexOf("Update isn’t completing")).toBeLessThan(text.indexOf("Unlock code"));
    });

    it("shows Home when the device or the child no longer exists", () => {
      render(<DevicePage childId="c1" deviceId="gone" />, makeStore([sam([laptop()])]));
      expect(container.textContent).toBe("HOME");
      render(<DevicePage childId="ghost" deviceId="d1" />, makeStore([sam([laptop()])]));
      expect(container.textContent).toBe("HOME");
    });
  });

  describe("the devices list", () => {
    it("puts one warn line on a device with a problem, and none on a healthy one", () => {
      const c = sam([laptop(), phone({ pairing: "unpaired" })]);
      render(<ChildDevices child={c} />, makeStore([c]));
      const rows = [...container.querySelectorAll("button.row")];
      expect(rows).toHaveLength(2);
      expect(rows[0].querySelector(".admin-warn")).toBeNull();
      expect(rows[1].querySelector(".admin-warn")?.textContent).toBe("Disconnected. Open it to reconnect or remove it.");
    });

    it("opens the device page, and offers to set up a device", () => {
      const c = sam([laptop()]);
      render(<ChildDevices child={c} />, makeStore([c]));
      act(() => (container.querySelector("button.row") as HTMLButtonElement).click());
      expect(window.location.hash).toBe("#/ward/c1/device/d1");
      click("Set up another device");
      expect(window.location.hash).toBe("#/ward/c1/add-device");
    });

    it("says 'Set up a device' when there are none", () => {
      const c = sam([]);
      render(<ChildDevices child={c} />, makeStore([c]));
      expect(button("Set up a device")).toBeTruthy();
    });

    it("deviceProblemLine ranks disconnected, failing update, gap, then health notes", () => {
      const now = 1_700_000_000_000;
      expect(deviceProblemLine(laptop({ pairing: "unpaired" }), undefined, now)).toMatch(/Disconnected/);
      expect(
        deviceProblemLine(laptop(), { updateHealth: { attempts: 3, sinceUnix: 1 } } as never, now),
      ).toMatch(/update isn’t completing/);
      expect(
        deviceProblemLine(laptop(), { enforcementGap: { unexplainedBoots: 1, lastNoticedAt: 1 } } as never, now),
      ).toMatch(/not running/);
      expect(deviceProblemLine(laptop(), undefined, now)).toBeNull();
    });
  });

  describe("settings for all of a child's devices", () => {
    it("Allow installs opens the window for the whole child", async () => {
      const c = sam([phone()]);
      const s = makeStore([c]);
      render(<ChildDeviceSettings child={c} />, s);
      expect(container.textContent).toContain("All of Sam’s devices");
      expect(container.textContent).toContain("every one of Sam’s devices");
      await act(async () => button("Allow installs").click());
      expect(s.openMaintenanceWindow).toHaveBeenCalledWith("c1", INSTALL_WINDOW_MINUTES);
      // The countdown starts, and "Close now" takes the button's place.
      expect(container.textContent).toContain("Installs are open");
      localStorage.clear();
      await act(async () => button("Close now").click());
      expect(s.openMaintenanceWindow).toHaveBeenLastCalledWith("c1", 0);
    });

    it("offers Update Kintrinsic only when a phone is behind the manifest", async () => {
      const c = sam([phone()]);
      const manifest = { versionCode: 60, versionName: "0.6.17", url: "/x.apk", sha256: "0".repeat(64) };
      const behind = makeStore([c], {
        updateManifest: manifest,
        deviceStatus: { [KEY2]: { appVersionCode: 50, appVersionName: "0.6.10" } },
      });
      render(<ChildDeviceSettings child={c} />, behind);
      await act(async () => button("Update Kintrinsic").click());
      expect(behind.sendCharterUpdate).toHaveBeenCalledWith("c1");
      expect(container.textContent).toContain("Update sent");

      const current = makeStore([c], {
        updateManifest: manifest,
        deviceStatus: { [KEY2]: { appVersionCode: 60, appVersionName: "0.6.17" } },
      });
      render(<ChildDeviceSettings child={c} />, current);
      expect(labels().some((l) => l?.includes("Update Kintrinsic"))).toBe(false);
    });

    it("renders nothing without an Android device or an update", () => {
      const c = sam([laptop()]);
      render(<ChildDeviceSettings child={c} />, makeStore([c]));
      expect(container.textContent).toBe("");
    });
  });

  describe("managing the child", () => {
    it("Remove asks first, then removes the child and goes Home", () => {
      const c = sam([laptop()]);
      const s = makeStore([c]);
      render(<ChildManage child={c} />, s);
      click("Remove Sam");
      expect(s.removeChild).not.toHaveBeenCalled();
      click("Remove Sam");
      expect(s.removeChild).toHaveBeenCalledWith("c1");
      expect(window.location.hash).toBe("#/home");
    });

    it("Rename saves the new name and colour", () => {
      const c = sam([]);
      const s = makeStore([c]);
      render(<ChildManage child={c} />, s);
      click("Rename or change colour");
      const input = container.querySelector("input.input") as HTMLInputElement;
      act(() => {
        const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
        set.call(input, "Sammy");
        input.dispatchEvent(new Event("input", { bubbles: true }));
      });
      act(() => (container.querySelectorAll('input[type="radio"]')[2] as HTMLInputElement).click());
      click("Save");
      expect(s.editChild).toHaveBeenCalledWith("c1", "Sammy", "#C4860F");
    });

    it("no longer teases game logins", () => {
      const c = sam([]);
      render(<ChildManage child={c} />, makeStore([c]));
      expect(container.textContent).not.toContain("game logins");
    });
  });

  describe("the add-device page", () => {
    it("shows Home for a child who no longer exists", () => {
      render(<AddDevicePage childId="ghost" />, makeStore([sam([])]));
      expect(container.textContent).toBe("HOME");
    });
  });
});
