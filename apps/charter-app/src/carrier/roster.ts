// Pure roster construction — no `window`, no bridge — so it is plain-Vitest
// testable without touching `window.CharterCarrier`. See `pushCarrierRoster`
// in ./bridge for the side-effecting half that actually hands this to the
// carrier shell.
//
// Why this exists: the carrier app classifies a gift-wrapped event down to a
// verdict carrying `machine` (the device's own pubkey) — it has no idea that
// pubkey is "Mia's Pixel 6", because names (Child.name, Device.label)
// live here, in the PWA. This is the PWA's half of the hand-off (design memo
// 2026-08-04, part 1).

import type { Child } from "../domain/types";
import { liveReleased } from "../store/releasedDevices";

export interface CarrierRosterEntry {
  /** The device's own pubkey — the same identity a carrier verdict's
   *  `machine` field carries, and the join key `describeWard` (carrier-side)
   *  looks up on. */
  machine: string;
  childName: string;
  deviceLabel: string;
  /**
   * True for `pairing === "paired"`. The carrier's local-network listener
   * admits only paired machines at the handshake (an entry with no `paired`
   * field, from an older console, means paired). A released machine is admitted
   * only for the `linkUntil` grace. Naming still covers devices
   * mid-pairing, so they stay in the list with `paired: false`.
   */
  paired: boolean;
  /**
   * True for a device the guardian disconnected (unpaired, removed, or its
   * child removed) in the last 14 days. Absent means false. A released entry
   * always has `paired: false`. It is listed so the carrier can name the device
   * and keep what it sends as a known sender; the carrier's local-network
   * listener still admits only `paired` machines.
   */
  released?: boolean;
  /**
   * Unix SECONDS the guardian disconnected the device. Present on a released
   * entry, and only there. The carrier compares a press with it (before or
   * after the disconnect) and drops the entry 14 days after it.
   */
  releasedAt?: number;
  /**
   * Epoch MILLISECONDS until which the carrier's local-network listener keeps
   * admitting this released machine, so its answer to the release (the
   * acknowledgement and the audits it held) can arrive over the home network.
   * Only on a released entry, and only while the grace runs; absent or in the
   * past means no link. The entry is never sent as paired for this.
   */
  linkUntil?: number;
}

/** What the carrier is told when a record has no name: it rejects an empty one. */
export const UNNAMED_CHILD = "Your child";
export const UNNAMED_DEVICE = "their phone";

/** A name or label, or its fallback when it is empty or blank. Never an empty string. */
function named(value: unknown, fallback: string): string {
  return typeof value === "string" && value.trim() !== "" ? value : fallback;
}

/**
 * One entry per device that has a pubkey. Includes devices mid-pairing or
 * mid-unpair — a notification can arrive from a device before/while its
 * `pairing` state changes, so this does NOT filter on `pairing === "paired"`.
 * A device with no pubkey yet has no `machine` to key a notification on and
 * is simply skipped, never padded with a placeholder.
 */
export function buildCarrierRoster(
  children: Child[],
  released: unknown = undefined,
  now: number = Date.now(),
  linkUntil: ReadonlyMap<string, number> = new Map(),
): CarrierRosterEntry[] {
  const roster: CarrierRosterEntry[] = [];
  for (const child of children) {
    for (const device of child.devices) {
      if (!device.devicePubkey) continue;
      roster.push({
        machine: device.devicePubkey,
        childName: named(child.name, UNNAMED_CHILD),
        deviceLabel: named(device.label, UNNAMED_DEVICE),
        paired: device.pairing === "paired",
      });
    }
  }
  // Devices disconnected in the last 14 days, so the carrier still names them
  // and keeps what they send. Never paired, and never listed twice: a machine
  // that is in the household again takes its household entry.
  const listed = new Set(roster.map((e) => e.machine));
  for (const r of liveReleased(released, now)) {
    if (listed.has(r.machine)) continue;
    listed.add(r.machine);
    // Always sent as released, never as paired. While the home-network link
    // grace runs (see ../store/releaseGrace) it also carries `linkUntil`, so the
    // carrier keeps the machine's session open for the release acknowledgement
    // and the audits it flushes, and lets it go at that time by itself.
    const link = linkUntil.get(r.machine);
    roster.push({
      machine: r.machine,
      childName: named(r.childName, UNNAMED_CHILD),
      deviceLabel: named(r.deviceLabel, UNNAMED_DEVICE),
      paired: false,
      released: true,
      releasedAt: Math.floor(r.releasedAt / 1000),
      ...(link !== undefined && link > now ? { linkUntil: link } : {}),
    });
  }
  return roster;
}
