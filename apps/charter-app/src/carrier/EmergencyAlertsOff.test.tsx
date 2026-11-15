import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import EmergencyAlertsOff from "./EmergencyAlertsOff";

const WARNING =
  "Emergency alerts are off on this phone. Turn them on so you’re told when your ward uses an emergency button.";

describe("EmergencyAlertsOff", () => {
  let container: HTMLDivElement;
  let root: Root;
  let state: { notifications: boolean; channel: boolean };
  let open: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    state = { notifications: true, channel: true };
    open = vi.fn();
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    delete window.CharterCarrier;
  });

  const mount = () => {
    act(() => {
      root = createRoot(container);
      root.render(<EmergencyAlertsOff />);
    });
  };
  const withShell = (extra: object = {}) => {
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      emergencyAlertState: () => JSON.stringify(state),
      openEmergencyAlertSettings: open,
      ...extra,
    } as typeof window.CharterCarrier;
  };
  const backToTheApp = () => {
    Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
  };

  it("shows nothing in a browser tab", () => {
    mount();
    expect(container.textContent).toBe("");
  });

  it("shows nothing with an older carrier that cannot report it", () => {
    withShell({ emergencyAlertState: undefined, openEmergencyAlertSettings: undefined });
    mount();
    expect(container.textContent).toBe("");
  });

  it("shows nothing when alerts are on", () => {
    withShell();
    mount();
    expect(container.textContent).toBe("");
  });

  it.each([
    ["notifications are off", { notifications: false, channel: true }],
    ["the Emergencies channel is off", { notifications: true, channel: false }],
    ["both are off", { notifications: false, channel: false }],
  ])("warns when %s, and the button opens the settings", (_name, s) => {
    state = s;
    withShell();
    mount();
    expect(container.textContent).toContain(WARNING);
    const button = container.querySelector("button")!;
    act(() => button.click());
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("checks again when the page becomes visible: the warning goes once alerts are on, and returns if they are turned off", () => {
    state = { notifications: false, channel: true };
    withShell();
    mount();
    expect(container.textContent).toContain(WARNING);
    state = { notifications: true, channel: true };
    backToTheApp();
    expect(container.textContent).toBe("");
    state = { notifications: true, channel: false };
    backToTheApp();
    expect(container.textContent).toContain(WARNING);
  });

  it("does not check while the page is hidden", () => {
    state = { notifications: false, channel: true };
    withShell();
    mount();
    state = { notifications: true, channel: true };
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    act(() => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    expect(container.textContent).toContain(WARNING);
    Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
  });
});
