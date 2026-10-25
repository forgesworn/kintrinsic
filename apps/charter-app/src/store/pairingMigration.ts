import type { Child } from "../domain/types";
import { DEFAULT_RELAYS, TROTTERS_RELAY } from "../signer/config";

/**
 * Add independent routes to legacy Trotters pairings once, mirroring both
 * wardens. Keep the configured relay; later explicit choices are honoured.
 */
export function backfillLegacyRelays(children: Child[]): { children: Child[]; changed: boolean } {
  let changed = false;
  const next = children.map((child) => {
    let childChanged = false;
    const devices = child.devices.map((device) => {
      if (device.pairing !== "paired" || device.relaysUpdatedAt !== undefined) return device;
      const existing = device.relays?.length ? device.relays : [TROTTERS_RELAY];
      const usesTrotters = existing.some((r) => {
        try { return new URL(r).hostname === new URL(TROTTERS_RELAY).hostname; }
        catch { return false; }
      });
      if (!usesTrotters) return device;
      childChanged = true;
      return { ...device, relays: [...new Set([...existing, ...DEFAULT_RELAYS])], relaysUpdatedAt: 0 };
    });
    if (!childChanged) return child;
    changed = true;
    return { ...child, devices };
  });
  return { children: changed ? next : children, changed };
}
