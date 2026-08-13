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

export interface CarrierRosterEntry {
  /** The device's own pubkey — the same identity a carrier verdict's
   *  `machine` field carries, and the join key `describeWard` (carrier-side)
   *  looks up on. */
  machine: string;
  childName: string;
  deviceLabel: string;
}

/**
 * One entry per device that has a pubkey. Includes devices mid-pairing or
 * mid-unpair — a notification can arrive from a device before/while its
 * `pairing` state changes, so this does NOT filter on `pairing === "paired"`.
 * A device with no pubkey yet has no `machine` to key a notification on and
 * is simply skipped, never padded with a placeholder.
 */
export function buildCarrierRoster(children: Child[]): CarrierRosterEntry[] {
  const roster: CarrierRosterEntry[] = [];
  for (const child of children) {
    for (const device of child.devices) {
      if (!device.devicePubkey) continue;
      roster.push({
        machine: device.devicePubkey,
        childName: child.name,
        deviceLabel: device.label,
      });
    }
  }
  return roster;
}
