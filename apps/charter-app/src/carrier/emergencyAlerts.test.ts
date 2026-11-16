import { afterEach, describe, expect, it, vi } from "vitest";
import { emergencyAlertsOff, openEmergencyAlertSettings, readEmergencyAlertState } from "./emergencyAlerts";

afterEach(() => {
  delete window.CharterCarrier;
});

const shell = (extra: Record<string, unknown>) => {
  window.CharterCarrier = { provision: vi.fn(), isCarrier: () => true, ...extra } as typeof window.CharterCarrier;
};

describe("readEmergencyAlertState", () => {
  it("is null in a browser tab, with no bridge at all", () => {
    expect(readEmergencyAlertState()).toBeNull();
  });
  it("is null with an older shell that lacks the method", () => {
    shell({});
    expect(readEmergencyAlertState()).toBeNull();
  });
  it("reads the shell's answer", () => {
    shell({ emergencyAlertState: () => JSON.stringify({ notifications: true, channel: false }) });
    expect(readEmergencyAlertState()).toEqual({ notifications: true, channel: false });
  });
  it("calls the method on the bridge object, never detached", () => {
    shell({
      emergencyAlertState(this: unknown) {
        if (this !== window.CharterCarrier) throw new Error("detached");
        return JSON.stringify({ notifications: true, channel: true });
      },
    });
    expect(readEmergencyAlertState()).toEqual({ notifications: true, channel: true });
  });
  it.each(["", "nope", "null", "[]", "{}", '{"notifications":true}', '{"notifications":"no","channel":true}'])(
    "treats an unreadable answer (%j) as unknown, never as off",
    (reply) => {
      shell({ emergencyAlertState: () => reply });
      expect(readEmergencyAlertState()).toBeNull();
    },
  );
  it("treats a throwing method as unknown", () => {
    shell({
      emergencyAlertState: () => {
        throw new Error("boom");
      },
    });
    expect(readEmergencyAlertState()).toBeNull();
  });
});

describe("emergencyAlertsOff", () => {
  it("is off when either is off, and never for an unknown answer", () => {
    expect(emergencyAlertsOff(null)).toBe(false);
    expect(emergencyAlertsOff({ notifications: true, channel: true })).toBe(false);
    expect(emergencyAlertsOff({ notifications: false, channel: true })).toBe(true);
    expect(emergencyAlertsOff({ notifications: true, channel: false })).toBe(true);
  });
});

describe("openEmergencyAlertSettings", () => {
  it("does nothing, and says so, without the method", () => {
    expect(openEmergencyAlertSettings()).toBe(false);
    shell({});
    expect(openEmergencyAlertSettings()).toBe(false);
  });
  it("calls it", () => {
    const open = vi.fn();
    shell({ openEmergencyAlertSettings: open });
    expect(openEmergencyAlertSettings()).toBe(true);
    expect(open).toHaveBeenCalledTimes(1);
  });
  it("survives a throw", () => {
    shell({
      openEmergencyAlertSettings: () => {
        throw new Error("boom");
      },
    });
    expect(openEmergencyAlertSettings()).toBe(false);
  });
});
