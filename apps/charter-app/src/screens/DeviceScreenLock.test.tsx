import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Child, Device } from "../domain/types";
import DeviceScreenLock from "./DeviceScreenLock";

const store = vi.hoisted(() => ({
  resetDeviceScreenLock: vi.fn(),
  deviceStatus: {} as Record<string, unknown>,
}));
vi.mock("../store/store", () => ({ useCharter: () => store }));

const PK = "a".repeat(64);
const ID = "0123456789abcdef0123456789abcdef";
const NOW = 1_800_000_000;
const phone: Device = { id: "d", label: "Robin’s phone", platform: "android", pairing: "paired", devicePubkey: PK };
const child = (d: Device): Child => ({ id: "c", name: "Robin", color: "#abc", dependantPubkey: null, devices: [d], policies: [] });
const status = (over: Record<string, unknown> = {}) => ({ appVersionCode: 56, screenLockSecure: true, ...over });

describe("Screen lock card", () => {
  let container: HTMLDivElement;
  let root: Root;
  function mount(device: Device) {
    act(() => {
      root = createRoot(container);
      root.render(<DeviceScreenLock child={child(device)} device={device} />);
    });
  }
  const buttons = () => [...container.querySelectorAll("button")];
  const button = (label: string) => buttons().find((x) => x.textContent === label || x.getAttribute("aria-label") === label);
  async function click(label: string) {
    const b = button(label);
    expect(b, label).toBeDefined();
    await act(async () => { b!.click(); });
  }
  /** Type into a React-controlled input the way a user would. */
  async function type(label: string, value: string) {
    const input = container.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
    expect(input, label).toBeTruthy();
    await act(async () => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }
  const text = () => container.textContent ?? "";

  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers({ toFake: ["Date", "setTimeout", "clearTimeout"] });
    vi.setSystemTime(NOW * 1000);
    store.deviceStatus = { [PK]: status() };
    store.resetDeviceScreenLock.mockResolvedValue(true);
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => { act(() => root.unmount()); container.remove(); vi.useRealTimers(); });

  it("shows 'Screen lock: on' and a Reset screen lock button", () => {
    mount(phone);
    expect(text()).toContain("Screen lock: on");
    expect(text()).not.toContain("Anyone holding this device");
    expect(button("Reset screen lock on Robin’s phone")?.disabled).toBe(false);
  });

  it("shows 'No screen lock' with the open-at-any-hour sentence", () => {
    store.deviceStatus = { [PK]: status({ screenLockSecure: false }) };
    mount(phone);
    expect(text()).toContain("No screen lock");
    expect(text()).toContain("Anyone holding this device can use its open-at-any-hour apps.");
  });

  it("renders nothing for an older ward, a laptop, or a status without screenLockSecure", () => {
    store.deviceStatus = { [PK]: status({ appVersionCode: 55 }) };
    mount(phone);
    expect(text()).toBe("");
    act(() => root.unmount());
    store.deviceStatus = { [PK]: status({ screenLockSecure: undefined }) };
    mount(phone);
    expect(text()).toBe("");
    act(() => root.unmount());
    store.deviceStatus = { [PK]: status({ appVersionCode: 9999 }) };
    mount({ ...phone, platform: "linux" });
    expect(text()).toBe("");
  });

  it("disables the button, with the reason, until the ward is ready", () => {
    store.deviceStatus = { [PK]: status({ screenLockResetReady: false }) };
    mount(phone);
    expect(button("Reset screen lock on Robin’s phone")?.disabled).toBe(true);
    expect(text()).toContain("Ready after Robin next unlocks this device.");
  });

  it("hides the button, and says why, when Android won't let Kintrinsic reset this phone", () => {
    const cannot =
      "Kintrinsic can't reset this device's screen lock. It had a screen lock before Kintrinsic was set up, and Android doesn't allow it after that. Setting the device up again from a factory reset fixes this.";
    store.deviceStatus = { [PK]: status({ screenLockResetReady: false, screenLockResetSupported: false }) };
    mount(phone);
    expect(button("Reset screen lock on Robin’s phone")).toBeUndefined();
    expect(text()).toContain("Screen lock: on");
    expect(text()).toContain(cannot);
    expect(text()).not.toContain("Ready after");
    act(() => root.unmount());
    // With no lock, the open-at-any-hour warning and the state line still show.
    store.deviceStatus = {
      [PK]: status({ screenLockSecure: false, screenLockResetReady: false, screenLockResetSupported: false }),
    };
    mount(phone);
    expect(button("Reset screen lock on Robin’s phone")).toBeUndefined();
    expect(text()).toContain("No screen lock");
    expect(text()).toContain("Anyone holding this device can use its open-at-any-hour apps.");
    expect(text()).toContain(cannot);
    act(() => root.unmount());
    // Supported: the button is back and there is no such sentence.
    store.deviceStatus = { [PK]: status({ screenLockResetSupported: true }) };
    mount(phone);
    expect(button("Reset screen lock on Robin’s phone")?.disabled).toBe(false);
    expect(text()).not.toContain("can't reset");
  });

  it("the dialog offers both choices and always says the ward's phone will tell them", async () => {
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    expect(text()).toContain("Set a new PIN");
    expect(text()).toContain("Remove the screen lock");
    expect(text()).toContain("Robin’s phone will tell them, and keep a record of it.");
    await click("Cancel");
    expect(container.querySelector("[role=dialog]")).toBeNull();
    // And again in the remove choice.
    await click("Reset screen lock on Robin’s phone");
    const remove = [...container.querySelectorAll<HTMLInputElement>("input[type=radio]")][1];
    await act(async () => { remove.click(); });
    expect(text()).toContain("Robin’s phone will tell them, and keep a record of it.");
  });

  it("sends a new PIN only when it is 4 to 16 digits and typed twice the same", async () => {
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    const send = () => button("Set the new PIN")!;
    expect(send().disabled).toBe(true);
    await type("New PIN", "123");
    await type("New PIN again", "123");
    expect(send().disabled).toBe(true); // too short
    await type("New PIN", "123456");
    await type("New PIN again", "123457");
    expect(send().disabled).toBe(true);
    expect(text()).toContain("The two PINs don’t match.");
    await type("New PIN again", "123456");
    expect(send().disabled).toBe(false);
    await click("Set the new PIN");
    expect(store.resetDeviceScreenLock).toHaveBeenCalledTimes(1);
    expect(store.resetDeviceScreenLock).toHaveBeenCalledWith("c", "d", "123456");
    expect(container.querySelector("[role=dialog]")).toBeNull();
  });

  it("takes digits only", async () => {
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    await type("New PIN", "12ab34");
    expect(container.querySelector<HTMLInputElement>('input[aria-label="New PIN"]')!.value).toBe("1234");
  });

  it("removes the lock with an empty PIN, and forgets what was typed", async () => {
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    await type("New PIN", "654321");
    const remove = [...container.querySelectorAll<HTMLInputElement>("input[type=radio]")][1];
    await act(async () => { remove.click(); });
    await click("Remove the screen lock");
    expect(store.resetDeviceScreenLock).toHaveBeenCalledWith("c", "d", "");
    // Opening it again shows an empty form: the PIN is gone, not kept.
    await click("Reset screen lock on Robin’s phone");
    expect(container.querySelector<HTMLInputElement>('input[aria-label="New PIN"]')!.value).toBe("");
  });

  it("says why a send failed, without the PIN", async () => {
    store.resetDeviceScreenLock.mockRejectedValue(new Error("Couldn't reach any relay — check your connection and try again."));
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    await type("New PIN", "135790");
    await type("New PIN again", "135790");
    await click("Set the new PIN");
    expect(text()).toContain("Couldn't reach any relay");
    expect(text()).not.toContain("135790");
  });

  it("asks the browser not to autofill or remember the PIN, and offers a number pad", async () => {
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    for (const label of ["New PIN", "New PIN again"]) {
      const input = container.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
      expect(input.getAttribute("autocomplete")).toBe("off");
      expect(input.getAttribute("inputmode")).toBe("numeric");
    }
  });

  it("says plainly when nothing was sent: no signer, or not paired", async () => {
    store.resetDeviceScreenLock.mockRejectedValue(
      new Error("Not sent: your signer isn't connected. Reconnect it and try again."),
    );
    mount(phone);
    await click("Reset screen lock on Robin’s phone");
    const remove = [...container.querySelectorAll<HTMLInputElement>("input[type=radio]")][1];
    await act(async () => { remove.click(); });
    await click("Remove the screen lock");
    expect(text()).toContain("Not sent: your signer isn't connected.");
  });

  it("says a held send is held for the home network", () => {
    mount({ ...phone, screenLockReset: { id: ID, issuedAt: NOW - 30, held: true } });
    expect(text()).toContain("Held until the device is on your home network.");
  });

  describe("after sending", () => {
    const sent = (issuedAt = NOW - 30) => ({ ...phone, screenLockReset: { id: ID, issuedAt } });
    const withResult = (result: string, at = new Date(2026, 10, 14, 19, 42).getTime() / 1000) =>
      (store.deviceStatus = { [PK]: status({ screenLockResets: [{ id: ID, result, at }] }) });

    it("waits for the device until its id turns up", () => {
      mount(sent());
      expect(text()).toContain("Waiting for the device…");
    });

    it("says Done at 19:42 once applied", () => {
      withResult("applied");
      mount(sent());
      expect(text()).toContain("Done at 19:42");
      expect(text()).not.toContain("Waiting for the device");
    });

    it.each([
      ["failed", "The device couldn't change the screen lock. Try again."],
      ["not_ready", "ready after Robin next unlocks it"],
      ["notice_failed", "couldn't tell Robin"],
      ["rejected_time", "its clock and yours disagree, or it arrived more than an hour late"],
      ["superseded", "A newer reset replaced this one."],
      ["accepted", "The device received it but hasn't confirmed the result yet."],
    ])("shows the plain words for %s", (result, words) => {
      withResult(result);
      mount(sent());
      expect(text()).toContain(words);
    });

    it("expires into the plain message after the hour, by itself", () => {
      mount(sent(NOW - 10));
      expect(text()).toContain("Waiting for the device…");
      act(() => { vi.setSystemTime((NOW + 3600) * 1000); vi.advanceTimersByTime(3600 * 1000); });
      expect(text()).toContain("The device didn't respond within an hour. Try again when it's online.");
      expect(text()).not.toContain("Waiting for the device");
    });

    it("a reset an hour old on mount is already expired", () => {
      mount(sent(NOW - 3600));
      expect(text()).toContain("The device didn't respond within an hour.");
    });
  });
});
