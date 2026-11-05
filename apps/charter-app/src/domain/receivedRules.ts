// Rules another guardian's phone set, on this phone's screens (decision 3,
// seeding). The roster carries the rules in force (`GuardianKeys`); the
// newest per (child, kind, device) is held there. What this module does with
// a received one that won:
//
//   - lossless (it reads back exactly, see `readClause`): it becomes this
//     phone's own rule for that kind, so the screen simply shows it;
//   - lossy: nothing here changes. The received clause stays the rule in
//     force, the screen shows it as set elsewhere (read-only, with Replace),
//     and a re-send of that kind sends the stored clause (`inForceToResend`).
//
// Seeding never sends anything to a ward: it changes what this install shows
// and what it would send next.

import { effectivePolicyForDevice, SPLITTABLE_CONTROLS, type SplittableControl } from "./effectivePolicy";
import type { Child, DeviceDebugging, Policy } from "./types";
import { readClause, SCREEN_KINDS, type ReadRule } from "../wire/clauseReverse";

/** A held rule as `GuardianKeys` keeps it: a signed inner CLAUSE event. */
export interface HeldRule {
  id: string;
  pubkey: string;
  content: string;
}

/** The device-scope policy control a clause kind fills. */
const CONTROL_OF: Record<string, keyof Policy> = {
  schedule: "schedule",
  budget: "budget",
  content: "web",
  apps: "apps",
  learning: "learning",
  tethering: "tethering",
  connections: "connections",
  lifeline: "lifeline",
  buckets: "buckets",
  listening: "listening",
  alwaysavailable: "alwaysAvailable",
  times: "times",
};

const isSplittable = (c: keyof Policy): c is SplittableControl => (SPLITTABLE_CONTROLS as readonly string[]).includes(c);

function payloadOf(e: HeldRule): { kind: string; issuedAt: number } & Record<string, unknown> {
  try {
    return JSON.parse(e.content) as { kind: string; issuedAt: number };
  } catch {
    return { kind: "", issuedAt: 0 };
  }
}

function machineOf(key: string): string | undefined {
  const at = key.indexOf("@");
  return at < 0 ? undefined : key.slice(at + 1);
}

function dropOverride(policy: Policy, deviceId: string, control: SplittableControl): Policy {
  const overrides = { ...(policy.deviceOverrides ?? {}) };
  const rest = { ...overrides[deviceId] };
  delete rest[control];
  if (Object.keys(rest).length > 0) overrides[deviceId] = rest;
  else delete overrides[deviceId];
  const out: Policy = { ...policy, deviceOverrides: overrides };
  if (Object.keys(overrides).length === 0) delete out.deviceOverrides;
  return out;
}

export interface SeedResult {
  /** The child's policies after seeding (the same array when nothing changed). */
  policies: Policy[];
  /** Devices whose USB-debugging record a received rule replaces. */
  debugging: { deviceId: string; debugging: DeviceDebugging }[];
  /** Keys dealt with (seeded, or left as set elsewhere), with their event ids. */
  applied: Record<string, string>;
}

/**
 * Put the received rules that won (`toApply`, by rule key) on this child's
 * screens. `held` is every rule held for the child (to tell which devices
 * keep a rule of their own). A device rule for a device this phone doesn't
 * know yet waits (it isn't marked applied); so does anything for a child
 * with no device-scope policy yet.
 */
export function seedChild(
  child: Child,
  toApply: Record<string, HeldRule>,
  held: Record<string, HeldRule>,
  mintPolicyId: () => string,
): SeedResult {
  let policies = child.policies;
  const debugging: SeedResult["debugging"] = [];
  const applied: Record<string, string> = {};
  // Family-wide rules first, so a device's own rule lands over its base.
  const keys = Object.keys(toApply).sort((a, b) => Number(a.includes("@")) - Number(b.includes("@")) || (a < b ? -1 : 1));
  for (const key of keys) {
    const event = toApply[key];
    const payload = payloadOf(event);
    const kind = payload.kind;
    const machine = machineOf(key);
    const device = machine ? child.devices.find((d) => d.devicePubkey === machine) : undefined;
    if (machine && !device) continue;
    if (!(SCREEN_KINDS as readonly string[]).includes(kind)) {
      applied[key] = event.id;
      continue;
    }
    const base = policies.find((p) => p.scope.kind === "device");
    if (kind !== "apprules" && !base) continue;
    const prior = base && device ? effectivePolicyForDevice(base, device.id) : base;
    const read = readClause(payload, { policy: prior });
    applied[key] = event.id;
    if (!read) continue;
    if (read.kind === "debugging") {
      if (device) debugging.push({ deviceId: device.id, debugging: { ...read.value, issuedAt: payload.issuedAt } });
      continue;
    }
    if (read.kind === "apprules") {
      policies = seedAppRules(policies, read, mintPolicyId);
      continue;
    }
    const control = CONTROL_OF[read.kind];
    if (!base || !control) continue;
    let next: Policy;
    if (device) {
      // A device rule for a control that can't be split isn't something this phone's screens hold.
      if (!isSplittable(control)) continue;
      const overrides = { ...(base.deviceOverrides ?? {}) };
      overrides[device.id] = { ...overrides[device.id], [control]: read.value };
      next = { ...base, deviceOverrides: overrides };
    } else {
      next = { ...base, [control]: read.value } as Policy;
      if (isSplittable(control)) {
        // Every device now has this rule, except one that holds a newer one of its own.
        for (const id of Object.keys(base.deviceOverrides ?? {})) {
          const d = child.devices.find((x) => x.id === id);
          if (d?.devicePubkey && held[`${kind}@${d.devicePubkey}`]) continue;
          if (next.deviceOverrides?.[id]?.[control] !== undefined) next = dropOverride(next, id, control);
        }
      }
    }
    policies = policies.map((p) => (p === base ? next : p));
  }
  return { policies, debugging, applied };
}

function seedAppRules(policies: Policy[], read: Extract<ReadRule, { kind: "apprules" }>, mintPolicyId: () => string): Policy[] {
  const apps = policies.filter((p) => p.scope.kind === "app");
  const others = policies.filter((p) => p.scope.kind !== "app");
  const seeded = read.value.map((a): Policy => {
    const existing = apps.find((p) => p.scope.kind === "app" && p.scope.appId === a.appId);
    const label = a.label || (existing?.scope.kind === "app" ? existing.scope.label : "") || a.appId;
    const out: Policy = { ...(existing ?? {}), id: existing?.id ?? mintPolicyId(), scope: { kind: "app", appId: a.appId, label }, blocked: a.blocked };
    if (a.schedule) out.schedule = a.schedule;
    else delete out.schedule;
    return out;
  });
  // Replace-the-set, as the clause is: an app no longer singled out goes.
  return [...others, ...seeded];
}

/** A rule in force that another phone set and this phone's screen can't show. */
export interface SetElsewhere {
  kind: string;
  /** The device it is for, when it went to one device only. */
  machine?: string;
  /** Who signed it (a root or a working key). */
  author: string;
  issuedAt: number;
}

/** The rule in force for one child, kind and device, as `GuardianKeys.ruleInForce` gives it. */
export type InForceLookup = (kind: string, machine?: string) => { event: HeldRule; local: boolean } | undefined;

/** Is this held rule one this phone's screen can't hold as it is? */
function lossy(event: HeldRule, prior: Policy | undefined): boolean {
  return readClause(payloadOf(event), { policy: prior }) === null;
}

/**
 * The rules among `kinds` that are in force for this child, were set on
 * another phone, and can't be shown on this phone's screens: family-wide,
 * and for each of the child's devices. A screen for one of these kinds is
 * read-only until the guardian replaces it.
 */
export function setElsewhere(child: Child, kinds: readonly string[], inForce: InForceLookup): SetElsewhere[] {
  const base = child.policies.find((p) => p.scope.kind === "device");
  const out: SetElsewhere[] = [];
  const seen = new Set<string>();
  const machines: (string | undefined)[] = [undefined, ...child.devices.map((d) => d.devicePubkey ?? undefined).filter((m): m is string => !!m)];
  for (const kind of kinds) {
    for (const machine of machines) {
      const r = inForce(kind, machine);
      if (!r || r.local || seen.has(r.event.id)) continue;
      const device = machine ? child.devices.find((d) => d.devicePubkey === machine) : undefined;
      const prior = base && device ? effectivePolicyForDevice(base, device.id) : base;
      if (!lossy(r.event, prior)) continue;
      seen.add(r.event.id);
      const key = machine && r.event.id !== inForce(kind)?.event.id ? machine : undefined;
      out.push({ kind, ...(key ? { machine: key } : {}), author: r.event.pubkey, issuedAt: payloadOf(r.event).issuedAt });
    }
  }
  return out;
}

/**
 * For a re-send of this child's rules (a newly set-up phone gets the whole
 * charter): the received clause to send for `kind` on `machine` instead of
 * this phone's own copy, when the rule in force there was set elsewhere and
 * this phone's screen can't hold it. Undefined otherwise.
 */
export function inForceToResend(child: Child, kind: string, machine: string, inForce: InForceLookup): HeldRule | undefined {
  const r = inForce(kind, machine);
  if (!r || r.local) return undefined;
  const base = child.policies.find((p) => p.scope.kind === "device");
  const device = child.devices.find((d) => d.devicePubkey === machine);
  const prior = base && device ? effectivePolicyForDevice(base, device.id) : base;
  return lossy(r.event, prior) ? r.event : undefined;
}
