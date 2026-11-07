// Pure glue between the household state and the guardian-keys layer
// (`keys/guardianKeys.ts`): the family as that layer sees it, the per-device
// view a STATUS leaves behind, roster ids, and the one check that keeps a
// family whose wards trust this phone's own key from "connecting My Signet
// as root" (that is a root move, not a connection).

import type { Child, Device } from "../domain/types";
import type { PresenceWard } from "../keys/state";
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
        ...(d.fromRoster ? { fromRoster: true } : {}),
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
 * departed and nothing more is sent to it, unless one of this install's own
 * roots (`ownRoots`) is still pinned there: then only the view is recorded,
 * and the root keeps signing for it. Never usage data: only the guardians
 * view and the departure are recorded. An older farewell than the view
 * already held changes nothing; the same array comes back unchanged.
 */
export function applyFarewell(children: Child[], farewell: StatusFarewell, w: string, ownRoots: readonly string[] = []): Child[] {
  const g = farewell.guardians;
  if (keyStanding(g, w).as !== "none") return children;
  const rootStays = g.roots.some((r) => ownRoots.includes(r.root));
  let changed = false;
  const next = children.map((c) => {
    if (!c.devices.some((d) => d.devicePubkey === farewell.machine)) return c;
    return {
      ...c,
      devices: c.devices.map((d) => {
        if (d.devicePubkey !== farewell.machine) return d;
        if (d.keysViewAt !== undefined && farewell.ts < d.keysViewAt) return d;
        if (d.departedAt !== undefined) return d;
        if (rootStays) {
          if (d.keysViewAt === farewell.ts && JSON.stringify(d.guardians ?? null) === JSON.stringify(g)) return d;
          changed = true;
          return { ...d, guardians: g, keysViewAt: farewell.ts };
        }
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
 * Record `key` as the pinned key of every paired device that has none yet.
 * Used only where the answer is still knowable: just before a phone that
 * signed with its own key connects My Signet (they pin the local key), and
 * when a family already on My Signet first approves this phone (devices not
 * heard from were offered by the root: `onlyUnheard`). The same array when
 * nothing changed.
 */
export function stampPinnedKeys(children: Child[], key: string, opts: { onlyUnheard?: boolean } = {}): Child[] {
  let changed = false;
  // A device this install has read a STATUS from sent it to the local key,
  // so it can't be pinned to the root: `onlyUnheard` leaves those unrecorded.
  // A device known only from another guardian's roster was paired by them:
  // what it pins isn't this install's to record.
  const eligible = (d: Child["devices"][number]) =>
    d.pairing === "paired" &&
    d.pinnedKey === undefined &&
    !d.fromRoster &&
    !(opts.onlyUnheard && (d.keysViewAt !== undefined || d.lastSeenAt !== undefined));
  const next = children.map((c) => {
    if (!c.devices.some(eligible)) return c;
    return {
      ...c,
      devices: c.devices.map((d) => {
        if (!eligible(d)) return d;
        changed = true;
        return { ...d, pinnedKey: key };
      }),
    };
  });
  return changed ? next : children;
}

/**
 * The devices that trust this phone's own key as their guardian, so that
 * connecting My Signet as root would leave them refusing everything R signs
 * (moving them is a root move). Fails closed: a device counts unless it is
 * known to hold the local key as a working key under some root (its STATUS
 * says so), or known to be pinned to another key (its recorded `pinnedKey`).
 * A device with no record counts whether or not it has been heard from.
 */
export function devicesPinningLocalKey(children: Child[], localKey: string): string[] {
  const out: string[] = [];
  for (const c of children) {
    for (const d of c.devices) {
      if (d.pairing !== "paired" || typeof d.devicePubkey !== "string" || !HEX64.test(d.devicePubkey)) continue;
      const standing = d.guardians ? keyStanding(d.guardians, localKey).as : "none";
      if (standing === "working") continue;
      if (standing === "root") {
        out.push(d.label);
        continue;
      }
      if (d.pinnedKey !== undefined && d.pinnedKey !== localKey) continue;
      // Another guardian paired it (it came in their roster): it was never
      // offered this phone's key.
      if (d.fromRoster) continue;
      out.push(d.label);
    }
  }
  return out;
}

/**
 * Take a merged family roster into the family (contract, *Family roster*):
 * for each child record that changed (`updated`, from `mergeRoster`):
 * - the child with that `rosterId` takes the record's name and colour (the
 *   merge already kept the newest `issuedAt`); a child that shares a device
 *   with the record under another `rosterId` is the same child, and both
 *   installs keep the smaller id, so they converge; otherwise a new child is
 *   added with no rules of its own;
 * - devices the record lists and the family lacks are added as known only
 *   from the roster (`fromRoster`), carrying what a presence ward's STATUS
 *   already said; their names follow the record;
 * - a device known only from a roster and never heard from is dropped when
 *   the record no longer lists it. A device paired here, or that has
 *   reported here, is never removed.
 * Returns the same array when nothing changed.
 */
export function adoptRoster(
  children: Child[],
  roster: {
    children: {
      subject: string;
      issuedAt: number;
      name: string;
      colour: string;
      devices: { machine: string; platform: string; name: string }[];
    }[];
  },
  updated: readonly string[],
  opts: {
    presenceWards?: Record<string, PresenceWard>;
    mintChildId: () => string;
    mintDeviceId: () => string;
    mintPolicyId: () => string;
  },
): Child[] {
  let next = children;
  const heard = (d: Device) => d.keysViewAt !== undefined || d.lastSeenAt !== undefined;
  for (const subject of updated) {
    const rc = roster.children.find((c) => c.subject === subject);
    if (!rc || !HEX64.test(rc.subject)) continue;
    const machines = new Set(rc.devices.filter((d) => HEX64.test(d.machine)).map((d) => d.machine));
    let idx = next.findIndex((c) => c.rosterId === subject);
    if (idx < 0) idx = next.findIndex((c) => c.devices.some((d) => typeof d.devicePubkey === "string" && machines.has(d.devicePubkey)));
    const before: Child =
      idx >= 0
        ? next[idx]
        : {
            id: opts.mintChildId(),
            name: rc.name,
            color: rc.colour,
            dependantPubkey: null,
            rosterId: subject,
            devices: [],
            policies: [{ id: opts.mintPolicyId(), scope: { kind: "device" } }],
          };
    const rosterId = before.rosterId && HEX64.test(before.rosterId) && before.rosterId < subject ? before.rosterId : subject;
    // Machines another child of this family already has stay where they are.
    const elsewhere = new Set(
      next
        .filter((_c, i) => i !== idx)
        .flatMap((c) => c.devices.map((d) => d.devicePubkey))
        .filter((m): m is string => typeof m === "string"),
    );
    const kept = before.devices
      .filter((d) => !(d.fromRoster && !heard(d) && typeof d.devicePubkey === "string" && !machines.has(d.devicePubkey)))
      .map((d) => {
        const rd = rc.devices.find((x) => x.machine === d.devicePubkey);
        return d.fromRoster && rd && rd.name !== d.label ? { ...d, label: rd.name } : d;
      });
    const have = new Set(kept.map((d) => d.devicePubkey));
    const added: Device[] = rc.devices
      .filter((rd) => HEX64.test(rd.machine) && !have.has(rd.machine) && !elsewhere.has(rd.machine))
      .filter((rd) => rd.platform === "android" || rd.platform === "linux")
      .map((rd) => {
        const p = opts.presenceWards?.[rd.machine];
        return {
          id: opts.mintDeviceId(),
          label: rd.name,
          platform: rd.platform as Device["platform"],
          pairing: "paired" as const,
          devicePubkey: rd.machine,
          fromRoster: true,
          ...(p?.ts !== undefined
            ? {
                keysViewAt: p.ts,
                ...(p.appVersionCode !== undefined ? { appVersionCode: p.appVersionCode } : {}),
                ...(p.guardians ? { guardians: p.guardians } : {}),
              }
            : {}),
        };
      });
    // Folding into a child held under another id: its own record, if newer, keeps its name.
    const own = idx >= 0 && before.rosterId !== subject ? roster.children.find((c) => c.subject === before.rosterId) : undefined;
    const keepName = own !== undefined && own.issuedAt >= rc.issuedAt;
    const after: Child = {
      ...before,
      name: keepName ? before.name : rc.name,
      color: keepName ? before.color : rc.colour,
      rosterId,
      devices: [...kept, ...added],
    };
    if (idx >= 0 && JSON.stringify(after) === JSON.stringify(before)) continue;
    next = idx >= 0 ? next.map((c, i) => (i === idx ? after : c)) : [...next, after];
  }
  return next;
}
