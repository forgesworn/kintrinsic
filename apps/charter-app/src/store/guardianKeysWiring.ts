// Pure glue between the household state and the guardian-keys layer
// (`keys/guardianKeys.ts`): the family as that layer sees it, the per-device
// view a STATUS leaves behind, roster ids, and the one check that keeps a
// family whose wards trust this phone's own key from "connecting My Signet
// as root" (that is a root move, not a connection).

import type { Child } from "../domain/types";
import type { KeysChild } from "../keys/guardianKeys";
import type { DeviceStatus, StatusFarewell } from "../wire/status";
import { deviceRelays } from "../signer/pairingRelays";
import { keyStanding } from "../keys/statusGuardians";

const HEX64 = /^[0-9a-f]{64}$/;

/** Every child with its paired, deliverable devices, as the keys layer reads them. */
export function keysChildren(children: Child[], defaultRelays: string[]): KeysChild[] {
  return children.map((c) => ({
    id: c.id,
    name: c.name,
    colour: c.color,
    ...(c.rosterId ? { rosterId: c.rosterId } : {}),
    devices: c.devices
      .filter(
        (d) =>
          d.pairing === "paired" &&
          d.departedAt === undefined &&
          typeof d.devicePubkey === "string" &&
          HEX64.test(d.devicePubkey),
      )
      .map((d) => ({
        machine: d.devicePubkey as string,
        platform: d.platform,
        name: d.label,
        relays: deviceRelays(d, defaultRelays),
        ...(d.appVersionCode !== undefined ? { appVersionCode: d.appVersionCode } : {}),
        ...(d.guardians ? { guardians: d.guardians } : {}),
        // A STATUS read here: what the gate counts. `keysViewAt` is set from
        // one; `lastSeenAt` covers devices heard from before this field.
        statusSeen: d.keysViewAt !== undefined || d.lastSeenAt !== undefined,
      })),
  }));
}

/**
 * Record what a STATUS said about the device's guardians and version. Only a
 * STATUS at least as new as the one already recorded replaces it; returns the
 * same array when nothing changed, so a repeat heartbeat doesn't churn.
 */
export function applyDeviceKeysView(children: Child[], status: DeviceStatus): Child[] {
  let changed = false;
  const next = children.map((c) => {
    if (!c.devices.some((d) => d.devicePubkey === status.machine)) return c;
    return {
      ...c,
      devices: c.devices.map((d) => {
        if (d.devicePubkey !== status.machine) return d;
        if (d.keysViewAt !== undefined && status.ts < d.keysViewAt) return d;
        const same =
          d.keysViewAt !== undefined &&
          d.departedAt === undefined &&
          d.appVersionCode === status.appVersionCode &&
          JSON.stringify(d.guardians ?? null) === JSON.stringify(status.guardians ?? null);
        if (same) return d;
        changed = true;
        // A full STATUS reaching this install means it is a recipient again.
        const { appVersionCode: _a, guardians: _g, departedAt: _d, ...rest } = d;
        void _a;
        void _g;
        void _d;
        return {
          ...rest,
          ...(status.appVersionCode !== undefined ? { appVersionCode: status.appVersionCode } : {}),
          ...(status.guardians ? { guardians: status.guardians } : {}),
          keysViewAt: Math.max(status.ts, d.keysViewAt ?? 0),
        };
      }),
    };
  });
  return changed ? next : children;
}

/**
 * Take in a departed key's last STATUS. When this install's working key `w`
 * is neither a root nor any root's working key in it, the device is marked
 * departed and nothing more is sent to it. Never usage data: only the
 * guardians view and the departure are recorded. An older farewell than the
 * view already held changes nothing; the same array comes back unchanged.
 */
export function applyFarewell(children: Child[], farewell: StatusFarewell, w: string): Child[] {
  const g = farewell.guardians;
  if (keyStanding(g, w).as !== "none") return children;
  let changed = false;
  const next = children.map((c) => {
    if (!c.devices.some((d) => d.devicePubkey === farewell.machine)) return c;
    return {
      ...c,
      devices: c.devices.map((d) => {
        if (d.devicePubkey !== farewell.machine) return d;
        if (d.keysViewAt !== undefined && farewell.ts < d.keysViewAt) return d;
        if (d.departedAt !== undefined) return d;
        changed = true;
        return { ...d, guardians: g, keysViewAt: farewell.ts, departedAt: farewell.ts };
      }),
    };
  });
  return changed ? next : children;
}

/** A fresh roster id: 32 random bytes, lowercase hex. */
export function newRosterId(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Give every child a roster id; the same array when all have one. */
export function ensureRosterIds(children: Child[], mint: () => string = newRosterId): { children: Child[]; changed: boolean } {
  if (children.every((c) => c.rosterId && HEX64.test(c.rosterId))) return { children, changed: false };
  return {
    children: children.map((c) => (c.rosterId && HEX64.test(c.rosterId) ? c : { ...c, rosterId: mint() })),
    changed: true,
  };
}

/**
 * The devices that trust this phone's own key as their guardian: ones whose
 * STATUS this install has read (so they send it to the local key) and that
 * don't hold the local key as a working key under some other root. For such
 * a family, connecting My Signet as root would leave those devices refusing
 * everything R signs; moving them is a root move.
 */
export function devicesPinningLocalKey(children: Child[], localKey: string): string[] {
  const out: string[] = [];
  for (const c of children) {
    for (const d of c.devices) {
      if (d.pairing !== "paired" || typeof d.devicePubkey !== "string" || !HEX64.test(d.devicePubkey)) continue;
      const heard = d.keysViewAt !== undefined || d.lastSeenAt !== undefined;
      if (!heard) continue;
      if (d.guardians && keyStanding(d.guardians, localKey).as === "working") continue;
      out.push(d.label);
    }
  }
  return out;
}
