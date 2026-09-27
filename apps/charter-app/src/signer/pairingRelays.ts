// Where a PAIRED device's traffic actually goes. A pairing's own `relays`
// (set at pairing time, never re-read from DEFAULT_RELAYS afterwards) is the
// source of truth; DEFAULT_RELAYS is only a fallback for a pairing that
// somehow carries none, and is always unioned in besides — a stale relay
// list must never make a device unreachable outright.

import type { Child, Device } from "../domain/types";

/** This one device's relays: its own stored list if it has one, else the
 *  caller's defaults. */
export function deviceRelays(device: Device, defaultRelays: string[]): string[] {
  return device.relays && device.relays.length > 0 ? device.relays : defaultRelays;
}

/**
 * Every relay any PAIRED device in the family might be listening on, plus the
 * current defaults — what polling (STATUS heartbeats + asks) must cover so a
 * device paired under an older default is never orphaned by a defaults
 * change.
 */
export function allPairingRelays(children: Child[], defaultRelays: string[]): string[] {
  const set = new Set(defaultRelays);
  for (const child of children) {
    for (const device of child.devices) {
      if (device.pairing !== "paired") continue;
      for (const r of deviceRelays(device, defaultRelays)) set.add(r);
    }
  }
  return [...set];
}
