import { describe, expect, it } from "vitest";
import { appsByDevice, appsBySection, appsFor, mergedApps } from "./deviceApps";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";

const LAPTOP_KEY = "a".repeat(64);
const PHONE_KEY = "c".repeat(64);

const laptop: Device = {
  id: "d_laptop",
  label: "Sam's laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: LAPTOP_KEY,
};
const phone: Device = {
  id: "d_phone",
  label: "Sam's phone",
  platform: "android",
  pairing: "paired",
  devicePubkey: PHONE_KEY,
};

/** Only the fields these selectors read; the rest of a STATUS is irrelevant. */
const status = (machine: string, apps: { pkg: string; label: string; userInstalled?: boolean }[]) =>
  ({ machine, apps }) as unknown as DeviceStatus;

const LAPTOP_APPS = [
  { pkg: "org.mozilla.firefox", label: "Firefox" },
  { pkg: "/usr/games/supertuxkart", label: "SuperTuxKart" },
];
const PHONE_APPS = [
  { pkg: "com.google.android.youtube", label: "YouTube" },
  { pkg: "com.roblox.client", label: "Roblox" },
];

const REPORTS: Record<string, DeviceStatus> = {
  [LAPTOP_KEY]: status(LAPTOP_KEY, LAPTOP_APPS),
  [PHONE_KEY]: status(PHONE_KEY, PHONE_APPS),
};

const pkgs = (list: { pkg: string }[]) => list.map((a) => a.pkg).sort();

describe("which apps a picker offers", () => {
  /**
   * The bug this exists to prevent: splitting a control per device showed the
   * SAME merged list under both devices, so the laptop's editor offered the
   * phone's Android packages and the phone's offered the laptop's binaries.
   * The warden matches a real flatpak id or exec path, so a rule set that way
   * can never once fire — and looks perfectly set to the guardian.
   */
  it("gives a split editor only the apps that device reported", () => {
    const by = appsByDevice([laptop, phone], REPORTS);
    const merged = mergedApps(by);

    expect(pkgs(appsFor(by, merged, laptop.id))).toEqual([
      "/usr/games/supertuxkart",
      "org.mozilla.firefox",
    ]);
    expect(pkgs(appsFor(by, merged, phone.id))).toEqual([
      "com.google.android.youtube",
      "com.roblox.client",
    ]);
  });

  it("never leaks one device's apps into the other's editor", () => {
    const by = appsByDevice([laptop, phone], REPORTS);
    const merged = mergedApps(by);
    const onLaptop = pkgs(appsFor(by, merged, laptop.id));
    const onPhone = pkgs(appsFor(by, merged, phone.id));
    for (const p of onPhone) expect(onLaptop).not.toContain(p);
    for (const p of onLaptop) expect(onPhone).not.toContain(p);
  });

  /** A rule that isn't split covers every device, so it offers every app. */
  it("gives a shared editor everything the ward has", () => {
    const by = appsByDevice([laptop, phone], REPORTS);
    expect(pkgs(appsFor(by, mergedApps(by), undefined))).toEqual([
      "/usr/games/supertuxkart",
      "com.google.android.youtube",
      "com.roblox.client",
      "org.mozilla.firefox",
    ]);
  });

  /**
   * Empty, NOT the merged list. The picker then says "no apps reported from the
   * device yet", which is true — a borrowed list would be a lie a guardian can
   * act on.
   */
  it("offers nothing for a device that has never reported", () => {
    const by = appsByDevice([laptop, phone], { [LAPTOP_KEY]: REPORTS[LAPTOP_KEY] });
    expect(appsFor(by, mergedApps(by), phone.id)).toEqual([]);
    // ...and an id that isn't a device at all.
    expect(appsFor(by, mergedApps(by), "d_nonexistent")).toEqual([]);
  });

  it("ignores devices that aren't paired or have no key", () => {
    const unpaired: Device = { ...phone, pairing: "unpaired" };
    const keyless: Device = { ...phone, id: "d_keyless", devicePubkey: undefined };
    const by = appsByDevice([laptop, unpaired, keyless], REPORTS);
    expect([...by.keys()]).toEqual([laptop.id]);
    expect(pkgs(mergedApps(by))).toEqual([
      "/usr/games/supertuxkart",
      "org.mozilla.firefox",
    ]);
  });

  it("dedupes an app both devices have, and sorts by label", () => {
    const shared = { pkg: "org.mozilla.firefox", label: "Firefox" };
    const by = appsByDevice([laptop, phone], {
      [LAPTOP_KEY]: status(LAPTOP_KEY, [shared, ...LAPTOP_APPS]),
      [PHONE_KEY]: status(PHONE_KEY, [shared, ...PHONE_APPS]),
    });
    const merged = mergedApps(by);
    expect(merged.filter((a) => a.pkg === "org.mozilla.firefox")).toHaveLength(1);
    expect(merged.map((a) => a.label)).toEqual([
      "Firefox",
      "Roblox",
      "SuperTuxKart",
      "YouTube",
    ]);
  });
});

describe("grouping the ward's inventory into per-device sections", () => {
  /** The whole point: a picker over the merged list can still say which
   *  device an app lives on. */
  it("gives one section per paired device, each with its own apps", () => {
    const sections = appsBySection([laptop, phone], REPORTS);
    expect(sections.map((s) => s.deviceId)).toEqual([laptop.id, phone.id]);
    expect(pkgs(sections[0].apps)).toEqual([
      "/usr/games/supertuxkart",
      "org.mozilla.firefox",
    ]);
    expect(pkgs(sections[1].apps)).toEqual([
      "com.google.android.youtube",
      "com.roblox.client",
    ]);
    expect(sections[0].label).toBe("Sam's laptop");
    expect(sections[1].label).toBe("Sam's phone");
  });

  /** The PWA already has a name for every device (Device.label) — reuse it
   *  verbatim. Only fall back when that's somehow unset. */
  it("falls back to a platform name when a device has no label", () => {
    const unnamedLaptop: Device = { ...laptop, label: "" };
    const unnamedPhone: Device = { ...phone, label: "   " };
    const sections = appsBySection([unnamedLaptop, unnamedPhone], REPORTS);
    expect(sections[0].label).toBe("Laptop");
    expect(sections[1].label).toBe("Phone");
  });

  /** A paired device that's never reported still gets a section — a picker
   *  needs its header to say so, not silently drop it. */
  it("gives a device with no reported apps an empty section rather than omitting it", () => {
    const sections = appsBySection([laptop, phone], {
      [LAPTOP_KEY]: REPORTS[LAPTOP_KEY],
    });
    expect(sections.map((s) => s.deviceId)).toEqual([laptop.id, phone.id]);
    expect(sections[1].apps).toEqual([]);
  });

  it("omits devices that aren't paired", () => {
    const unpaired: Device = { ...phone, pairing: "unpaired" };
    const sections = appsBySection([laptop, unpaired], REPORTS);
    expect(sections.map((s) => s.deviceId)).toEqual([laptop.id]);
  });

  /**
   * The grouped view is not the merged one: identities are platform-specific
   * so this is rare in practice, but an id both devices happen to report must
   * stay in EACH section rather than being deduped away like `mergedApps`
   * would. A guardian scanning the phone's section should see everything the
   * phone reported, full stop.
   */
  it("keeps an app that appears on two devices in both sections, not deduped", () => {
    const shared = { pkg: "org.mozilla.firefox", label: "Firefox" };
    const sections = appsBySection([laptop, phone], {
      [LAPTOP_KEY]: status(LAPTOP_KEY, [shared, ...LAPTOP_APPS]),
      [PHONE_KEY]: status(PHONE_KEY, [shared, ...PHONE_APPS]),
    });
    expect(pkgs(sections[0].apps)).toContain("org.mozilla.firefox");
    expect(pkgs(sections[1].apps)).toContain("org.mozilla.firefox");
  });
});

// Honest attribution (2026-08-03): AppRef.userInstalled flags a ward-writable
// inventory entry (their own `.local/share/applications`, or a `--user`
// flatpak) — the picker's only way to say "installed by <ward>" rather than
// implying an entry the ward has no way to have altered.
describe("userInstalled carries through appsBySection", () => {
  const prismUserInstalled = { pkg: "org.prismlauncher.PrismLauncher", label: "Prism Launcher", userInstalled: true };

  it("keeps userInstalled: true on a ward-writable entry", () => {
    const sections = appsBySection([laptop], {
      [LAPTOP_KEY]: status(LAPTOP_KEY, [prismUserInstalled, ...LAPTOP_APPS]),
    });
    const prism = sections[0].apps.find((a) => a.pkg === prismUserInstalled.pkg);
    expect(prism?.userInstalled).toBe(true);
  });

  it("leaves a root-owned entry's userInstalled absent, never coerced to false", () => {
    const sections = appsBySection([laptop], { [LAPTOP_KEY]: status(LAPTOP_KEY, LAPTOP_APPS) });
    const firefox = sections[0].apps.find((a) => a.pkg === "org.mozilla.firefox");
    expect(firefox?.userInstalled).toBeUndefined();
    expect(firefox).not.toHaveProperty("userInstalled");
  });
});
