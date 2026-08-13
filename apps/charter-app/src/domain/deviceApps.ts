import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";

/** One entry from a device's reported inventory: its on-device identity and a
 *  human label. `pkg` is an Android package id, a flatpak id, or an exec path. */
export interface ReportedApp {
  pkg: string;
  label: string;
  /** true only when this entry came from a ward-writable scan dir (the
   *  managed user's own `.local/share/applications` or a `--user` flatpak) —
   *  see `AppRef.userInstalled`. Absent (root-owned) otherwise; never
   *  coerced to `false`, so a picker can tell "root-owned" from "unknown". */
  userInstalled?: boolean;
}

/**
 * Which apps a picker should offer, per device and merged.
 *
 * The distinction is load-bearing. A rule that covers the whole ward should
 * offer everything the ward has; a rule SPLIT per device must offer only what
 * that device actually reported. Handing the merged list to a split editor put
 * the phone's Android packages under the laptop and the laptop's binaries under
 * the phone (decented, 2026-08-01) — and the warden matches a real flatpak id or
 * exec path, so "block com.android.chrome on the laptop" writes a rule that can
 * never once fire while looking perfectly set. Silently inert rules are the
 * worst failure this screen has.
 */
export function appsByDevice(
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus>,
): Map<string, ReportedApp[]> {
  const out = new Map<string, ReportedApp[]>();
  for (const d of devices) {
    // Only a paired device with a real key can have reported anything.
    if (d.pairing !== "paired" || !d.devicePubkey) continue;
    out.set(d.id, dedupeSort(deviceStatus[d.devicePubkey]?.apps ?? []));
  }
  return out;
}

/** Everything the ward has, across all their devices — the right list for a
 *  rule that isn't split. */
export function mergedApps(byDevice: Map<string, ReportedApp[]>): ReportedApp[] {
  const all: ReportedApp[] = [];
  for (const list of byDevice.values()) all.push(...list);
  return dedupeSort(all);
}

/**
 * What THIS editor should offer: one device's own inventory when the control is
 * split, everything the ward has when it isn't.
 *
 * A device that has never reported returns an empty list rather than falling
 * back to the merged one — the pickers say "no apps reported from the device
 * yet", which is true, where a borrowed list would be a lie you can act on.
 */
export function appsFor(
  byDevice: Map<string, ReportedApp[]>,
  merged: ReportedApp[],
  deviceId?: string,
): ReportedApp[] {
  return deviceId === undefined ? merged : byDevice.get(deviceId) ?? [];
}

/** One paired device's own slice of a picker, headed by its name. */
export interface AppSection {
  deviceId: string;
  /** The same name the ward/device list shows for this device (Device.label),
   *  with a fallback chain for the case it somehow arrives unset. */
  label: string;
  platform: Device["platform"];
  apps: ReportedApp[];
}

/**
 * The ward's reported inventory, grouped by paired device rather than merged —
 * for any picker that lists every app across the whole ward but must still say
 * which device each one lives on. Mixing a laptop's flatpak ids in with a
 * phone's Android packages in one flat list reads as one inventory when it's
 * really two unrelated vocabularies (see the module doc above); grouping is
 * presentation only; it does not change what a shared (unsplit) rule matches.
 *
 * Order follows `devices`; a device's own list keeps whatever order/dedup
 * `appsByDevice` already gave it. An app id that (rarely — identities are
 * platform-specific) appears on two devices appears in both sections: this is
 * the grouped view, not the merged one, so nothing here dedupes across a
 * device boundary.
 */
export function appsBySection(
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus>,
): AppSection[] {
  const byDevice = appsByDevice(devices, deviceStatus);
  return devices
    .filter((d) => d.pairing === "paired")
    .map((d) => ({
      deviceId: d.id,
      label: deviceSectionLabel(d),
      platform: d.platform,
      apps: byDevice.get(d.id) ?? [],
    }));
}

function deviceSectionLabel(d: Device): string {
  const named = d.label?.trim();
  if (named) return named;
  if (d.platform === "android") return "Phone";
  if (d.platform === "linux") return "Laptop";
  return d.id.length > 10 ? `${d.id.slice(0, 10)}…` : d.id;
}

function dedupeSort(apps: readonly ReportedApp[]): ReportedApp[] {
  const byPkg = new Map<string, ReportedApp>();
  for (const a of apps) if (!byPkg.has(a.pkg)) byPkg.set(a.pkg, a);
  return [...byPkg.values()].sort((a, b) => a.label.localeCompare(b.label));
}
