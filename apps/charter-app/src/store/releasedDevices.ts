// Devices the guardian has disconnected, remembered for a short grace period.
//
// Why: when a guardian disconnects a phone (or removes it, or removes its
// child), the ward answers the release by sending every audit it was still
// holding (the contract's "A guardian's release and the held audits"). A press
// of the emergency button made while the phone was out of reach is therefore
// reported AFTER the console has forgotten the device. Without a record, the
// console would call that press a stranger's and the guardian would never see
// it in Activity.
//
// What a released machine may do is deliberately narrow. It is NOT paired, and
// nothing treats it as paired. The only thing honoured from it is a break-glass
// or emergency-call audit, so the guardian is told about a press made before
// the disconnect. No asks, no status, no clause delivery, no local-network
// admission.

import type { Child, Device } from "../domain/types";

/** How long a disconnected device stays on record. */
export const RELEASED_GRACE_MS = 14 * 24 * 60 * 60 * 1000;

/** Never keep more than this many; the newest win. */
const MAX_RELEASED = 50;

export interface ReleasedDevice {
  /** The device's own pubkey: the join key an audit's `machine` carries. */
  machine: string;
  /** The child it belonged to. May no longer exist (the child can be removed). */
  childId: string;
  /** Kept so a line can name the child after the child is gone. */
  childName: string;
  deviceLabel: string;
  /** The device's own relays, so the flush can still be heard on them. */
  relays: string[];
  /** Epoch ms the guardian disconnected it. */
  releasedAt: number;
}

/**
 * Read the persisted list defensively: it came out of localStorage, and an
 * older household has no such field at all (that is an empty list).
 */
export function releasedOf(list: unknown): ReleasedDevice[] {
  if (!Array.isArray(list)) return [];
  const out: ReleasedDevice[] = [];
  for (const r of list) {
    if (typeof r !== "object" || r === null) continue;
    const x = r as Partial<ReleasedDevice>;
    if (
      typeof x.machine !== "string" ||
      x.machine === "" ||
      typeof x.childId !== "string" ||
      typeof x.releasedAt !== "number" ||
      !Number.isFinite(x.releasedAt)
    ) {
      continue;
    }
    out.push({
      machine: x.machine,
      childId: x.childId,
      childName: typeof x.childName === "string" ? x.childName : "",
      deviceLabel: typeof x.deviceLabel === "string" ? x.deviceLabel : "the device",
      relays: Array.isArray(x.relays) ? x.relays.filter((u): u is string => typeof u === "string") : [],
      releasedAt: x.releasedAt,
    });
  }
  return out;
}

/** The records still inside their grace period (never one dated in the future). */
export function liveReleased(list: unknown, now: number): ReleasedDevice[] {
  return releasedOf(list).filter((r) => now - r.releasedAt <= RELEASED_GRACE_MS && r.releasedAt <= now + 60_000);
}

/** The live record for a machine, if the guardian disconnected it recently. */
export function findReleased(list: unknown, machine: string, now: number): ReleasedDevice | undefined {
  return liveReleased(list, now).find((r) => r.machine === machine);
}

/**
 * Add a record for each of `devices` that has a pubkey, replacing any older
 * record for the same machine and pruning what has expired. Returns the new
 * list.
 */
export function withReleased(
  list: unknown,
  child: Pick<Child, "id" | "name">,
  devices: Pick<Device, "label" | "devicePubkey" | "relays">[],
  now: number,
): ReleasedDevice[] {
  const added: ReleasedDevice[] = [];
  for (const d of devices) {
    if (typeof d.devicePubkey !== "string" || d.devicePubkey === "") continue;
    added.push({
      machine: d.devicePubkey,
      childId: child.id,
      childName: child.name,
      deviceLabel: d.label,
      relays: d.relays ? [...d.relays] : [],
      releasedAt: now,
    });
  }
  const machines = new Set(added.map((a) => a.machine));
  const kept = liveReleased(list, now).filter((r) => !machines.has(r.machine));
  return [...kept, ...added].slice(-MAX_RELEASED);
}

/** Forget a machine (it has been paired again). Returns the same list when nothing changed. */
export function withoutReleased(list: unknown, machine: string): ReleasedDevice[] | undefined {
  const cur = releasedOf(list);
  const next = cur.filter((r) => r.machine !== machine);
  return next.length === cur.length ? undefined : next;
}

/** The relays the carrier and the poll keep listening on for released devices. */
export function releasedRelays(list: unknown, now: number): string[] {
  const set = new Set<string>();
  for (const r of liveReleased(list, now)) for (const u of r.relays) set.add(u);
  return [...set];
}
