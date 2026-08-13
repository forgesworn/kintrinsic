import { describe, it, expect, vi } from "vitest";
import { grantUsageAccess, preflight, setDeviceOwner, pairOverCable, runProvision } from "./provision";
import type { AdbSession } from "./webusb";
import { ProvisionError } from "./webusb";

function fakeSession(shellImpl: (cmd: string) => { stdout: string; exitCode: number }): AdbSession {
  return {
    serial: "TEST",
    shell: vi.fn(async (cmd: string) => shellImpl(cmd)),
    pushFile: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
  };
}

describe("preflight", () => {
  it("rejects a phone that already has accounts", async () => {
    const s = fakeSession((cmd) =>
      cmd.includes("dumpsys account") ? { stdout: "Account {name=foo@gmail.com}", exitCode: 0 }
                                      : { stdout: "", exitCode: 0 });
    await expect(preflight(s)).rejects.toMatchObject({ code: "precondition" });
    await expect(preflight(s)).rejects.toBeInstanceOf(ProvisionError);
  });

  it("passes a clean phone", async () => {
    const s = fakeSession(() => ({ stdout: "", exitCode: 0 }));
    await expect(preflight(s)).resolves.toBeUndefined();
  });
});

describe("setDeviceOwner", () => {
  it("throws with remediation when dpm refuses", async () => {
    const s = fakeSession(() => ({ stdout: "java.lang.IllegalStateException: Not allowed to set the device owner because there are already several users", exitCode: 0 }));
    await expect(setDeviceOwner(s)).rejects.toMatchObject({ code: "adb" });
  });

  it("succeeds on the success banner", async () => {
    const s = fakeSession(() => ({ stdout: "Success: Device owner set to package org.forgesworn.charter", exitCode: 0 }));
    await expect(setDeviceOwner(s)).resolves.toBeUndefined();
  });

  it("matches remediation text even when it originated on stderr (Fix 1: AdbSession.shell combines stdout+stderr)", async () => {
    // On a real shell_v2 device, `dpm set-device-owner` refusals print to
    // STDERR, not stdout — webusb.ts's wrapSession.shell() folds both into
    // the single `stdout` string this test simulates. Before that fix, a
    // refusal like this would have shown up here as an empty string and the
    // parent would have gotten a blank error reason.
    const s = fakeSession(() => ({
      stdout: "java.lang.IllegalStateException: Not allowed to set the device owner because there are already some accounts on the device",
      exitCode: 1,
    }));
    await expect(setDeviceOwner(s)).rejects.toMatchObject({ code: "adb" });
    await expect(setDeviceOwner(s)).rejects.toThrow(/factory-reset/i);
  });
});

describe("pairOverCable", () => {
  it("fires the bunker URI via am start", async () => {
    const calls: string[] = [];
    const s = fakeSession((cmd) => { calls.push(cmd); return { stdout: "Starting: Intent", exitCode: 0 }; });
    await pairOverCable(s, "bunker://relay?pubkey=abc&token=xyz");
    expect(calls.some((c) => c.includes("am start") && c.includes("bunker://"))).toBe(true);
  });
});

describe("grantUsageAccess", () => {
  // GET_USAGE_STATS is an appop, not a runtime permission: `pm install -g`
  // does NOT grant it, and production's setPermissionGrantState can return
  // false silently. Ungranted, the daily-limit meter cannot accrue and every
  // limit is silently toothless (bramble issue #42; oriole again 2026-07-27).
  // The dev cable script grants + verifies; the parent-facing flow must too —
  // the cable is the one moment a human is present to fix it.
  it("grants the usage-access op and verifies it took", async () => {
    const calls: string[] = [];
    const s = fakeSession((cmd) => {
      calls.push(cmd);
      if (cmd.includes("appops get")) {
        return { stdout: "GET_USAGE_STATS: allow", exitCode: 0 };
      }
      return { stdout: "", exitCode: 0 };
    });
    await expect(grantUsageAccess(s)).resolves.toBeUndefined();
    expect(
      calls.some((c) => c.includes("appops set") && c.includes("android:get_usage_stats allow")),
    ).toBe(true);
  });

  it("fails loudly when the grant did not take", async () => {
    const s = fakeSession((cmd) =>
      cmd.includes("appops get")
        ? { stdout: "GET_USAGE_STATS: ignore", exitCode: 0 }
        : { stdout: "", exitCode: 0 },
    );
    await expect(grantUsageAccess(s)).rejects.toBeInstanceOf(ProvisionError);
    await expect(grantUsageAccess(s)).rejects.toThrow(/usage access/i);
  });
});

describe("runProvision", () => {
  it("walks all steps and reports progress", async () => {
    const s = fakeSession((cmd) => {
      if (cmd.includes("set-device-owner")) return { stdout: "Success: Device owner set", exitCode: 0 };
      if (cmd.includes("pm install")) return { stdout: "Success", exitCode: 0 };
      if (cmd.includes("appops get")) return { stdout: "GET_USAGE_STATS: allow", exitCode: 0 };
      return { stdout: "", exitCode: 0 };
    });
    const steps: string[] = [];
    await runProvision(s, new Uint8Array([1, 2, 3]), "bunker://r?token=t",
      (p) => steps.push(`${p.step}:${p.ok}`));
    expect(steps).toContain("done:true");
    // The meter grant is part of the walk — a phone provisioned by a parent
    // must not come out with silently toothless limits.
    expect(steps).toContain("grant:true");
  });
});
