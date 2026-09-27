// One-time back-fill: a pairing made before `Device.relays` existed carries
// no relay list at all. Rather than silently falling onto whatever
// DEFAULT_RELAYS happens to be today (which, after a defaults change, may
// share nothing with the relay that device is actually listening on — see
// the trotters.cc removal), it is stamped with LEGACY_PAIRING_RELAYS — the
// one relay every pairing used before this field existed — once, the first
// time it is loaded. This is a record of what that pairing already was, not
// a default, and it is never applied to a new pairing.

import type { Child } from "../domain/types";
import { LEGACY_PAIRING_RELAYS } from "../signer/config";

/**
 * Back-fill `relays` on every PAIRED device missing it. Idempotent: a device
 * that already carries a `relays` list (new-style pairing, or already
 * migrated) is returned unchanged, and re-running finds nothing left to do.
 */
export function backfillLegacyRelays(children: Child[]): { children: Child[]; changed: boolean } {
  let changed = false;
  const next = children.map((child) => {
    let childChanged = false;
    const devices = child.devices.map((device) => {
      if (device.pairing === "paired" && (!device.relays || device.relays.length === 0)) {
        childChanged = true;
        return { ...device, relays: LEGACY_PAIRING_RELAYS };
      }
      return device;
    });
    if (!childChanged) return child;
    changed = true;
    return { ...child, devices };
  });
  return { children: changed ? next : children, changed };
}
