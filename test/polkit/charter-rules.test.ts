// Evaluates the REAL polkit rules file (linux/packaging/polkit/49-charter.rules)
// in a sandbox with a mock `polkit` global, and asserts the allow/deny matrix.
//
// Why this exists: the rules once blanket-denied the whole
// `org.freedesktop.login1.` prefix, which silently swallowed plain
// power-off/suspend/hibernate — a ward could not shut his own laptop down
// without logging out first. Normal power actions must stay NOT_HANDLED
// (distro default: allowed for an active local session); only the actual
// escape hatches stay NO.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

const RULES_PATH = fileURLToPath(
  new URL("../../linux/packaging/polkit/49-charter.rules", import.meta.url),
);

const NO = "no";
const NOT_HANDLED = "not_handled";

type RuleFn = (
  action: { id: string; lookup?: (k: string) => unknown },
  subject: { isInGroup: (g: string) => boolean },
) => unknown;

/** Load the rules file the way polkitd would: collect addRule callbacks. */
function loadRules(): RuleFn[] {
  const rules: RuleFn[] = [];
  const polkit = {
    Result: { NO, YES: "yes", NOT_HANDLED, AUTH_ADMIN: "auth_admin" },
    addRule: (fn: RuleFn) => rules.push(fn),
    log: () => {},
  };
  runInNewContext(readFileSync(RULES_PATH, "utf8"), { polkit });
  return rules;
}

/** First non-NOT_HANDLED verdict wins, like polkitd's rule chain. */
function verdict(actionId: string, groups: string[]): string {
  const subject = { isInGroup: (g: string) => groups.includes(g) };
  for (const rule of loadRules()) {
    const r = rule({ id: actionId }, subject);
    if (r !== undefined && r !== NOT_HANDLED) return r as string;
  }
  return NOT_HANDLED;
}

const ward = ["charter-managed"];

// Plain power actions a ward's desktop needs. NOT_HANDLED = the distro
// default applies (allowed for an active local session on Mint).
const ALLOWED_POWER = [
  "org.freedesktop.login1.power-off",
  "org.freedesktop.login1.power-off-multiple-sessions",
  "org.freedesktop.login1.reboot",
  "org.freedesktop.login1.reboot-multiple-sessions",
  "org.freedesktop.login1.halt",
  "org.freedesktop.login1.halt-multiple-sessions",
  "org.freedesktop.login1.suspend",
  "org.freedesktop.login1.suspend-multiple-sessions",
  "org.freedesktop.login1.hibernate",
  "org.freedesktop.login1.hibernate-multiple-sessions",
];

// Inhibitor grabs Cinnamon's own daemons (csd-power, csd-media-keys) and
// ordinary apps (video players) take from inside the ward session. Denying
// these breaks lid-switch/power-key handling for the whole desktop.
const ALLOWED_INHIBIT = [
  "org.freedesktop.login1.inhibit-delay-sleep",
  "org.freedesktop.login1.inhibit-delay-shutdown",
  "org.freedesktop.login1.inhibit-block-sleep",
  "org.freedesktop.login1.inhibit-block-idle",
  "org.freedesktop.login1.inhibit-handle-power-key",
  "org.freedesktop.login1.inhibit-handle-suspend-key",
  "org.freedesktop.login1.inhibit-handle-hibernate-key",
  "org.freedesktop.login1.inhibit-handle-lid-switch",
];

// The actual login1 escape hatches: overriding inhibitors, rebooting into
// firmware/boot-loader, lingering, device attach, VT hopping.
const BLOCKED_LOGIN1 = [
  "org.freedesktop.login1.power-off-ignore-inhibit",
  "org.freedesktop.login1.reboot-ignore-inhibit",
  "org.freedesktop.login1.halt-ignore-inhibit",
  "org.freedesktop.login1.suspend-ignore-inhibit",
  "org.freedesktop.login1.hibernate-ignore-inhibit",
  "org.freedesktop.login1.inhibit-block-shutdown",
  "org.freedesktop.login1.set-reboot-to-firmware-setup",
  "org.freedesktop.login1.set-reboot-to-boot-loader-menu",
  "org.freedesktop.login1.set-reboot-to-boot-loader-entry",
  "org.freedesktop.login1.set-user-linger",
  "org.freedesktop.login1.attach-device",
  "org.freedesktop.login1.flush-devices",
  "org.freedesktop.login1.chvt",
  "org.freedesktop.login1.lock-sessions",
  "org.freedesktop.login1.set-wall-message",
];

// The non-login1 admin surfaces that must stay denied for a ward.
const BLOCKED_OTHER = [
  "org.freedesktop.Flatpak.app-install",
  "org.freedesktop.packagekit.package-install",
  "org.freedesktop.timedate1.set-time",
  "org.freedesktop.policykit.exec",
  "org.freedesktop.systemd1.manage-units",
  "org.freedesktop.accounts.change-own-user-data",
];

describe("49-charter.rules", () => {
  it("lets a ward power off / reboot / suspend / hibernate (distro default applies)", () => {
    for (const id of ALLOWED_POWER) {
      expect(verdict(id, ward), id).toBe(NOT_HANDLED);
    }
  });

  it("lets the ward session take ordinary inhibitor locks (lid, power key, media)", () => {
    for (const id of ALLOWED_INHIBIT) {
      expect(verdict(id, ward), id).toBe(NOT_HANDLED);
    }
  });

  it("still denies the ward the login1 escape hatches", () => {
    for (const id of BLOCKED_LOGIN1) {
      expect(verdict(id, ward), id).toBe(NO);
    }
  });

  it("still denies the ward the non-login1 admin surfaces", () => {
    for (const id of BLOCKED_OTHER) {
      expect(verdict(id, ward), id).toBe(NO);
    }
  });

  it("does not touch anyone outside charter-managed", () => {
    for (const id of [...ALLOWED_POWER, ...BLOCKED_LOGIN1, ...BLOCKED_OTHER]) {
      expect(verdict(id, ["sudo", "adm"]), id).toBe(NOT_HANDLED);
    }
  });
});
