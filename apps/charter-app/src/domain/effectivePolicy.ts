// One charter, split where it fits the child (per-device rules, Half A of
// docs/superpowers/specs/2026-07-24-per-device-rules-design.md).
//
// A child has one base charter that applies to every device they use. For any
// individual control a family can say "set this separately for this device" —
// the homework laptop genuinely differs from the fun phone, and that is fitting
// a family rather than tightening a net. Everything is SHARED by default, so a
// child who has never been split is byte-identical to before this existed.
//
// Splitting happens at CONTROL granularity (the line item), not whole-device
// profiles: it maps onto the independent clauses we already publish, and it
// keeps the model something a parent can hold in their head.
//
// This is Kintrinsic-only. Transport and both wardens have always been
// per-device — each device has its own key and sees only clauses wrapped to it
// — so no wire or warden change is needed. The one hard rule is that
// `deviceOverrides` is OUR bookkeeping and must never reach the wire; see
// `effectivePolicyForDevice`.

import type { Policy } from "./types";

/**
 * The controls a family can split. Hotspot and lifeline are deliberately
 * absent: they exist only on a phone, so they are already per-device by
 * nature and splitting them would be a setting with nothing on the other side.
 */
export const SPLITTABLE_CONTROLS = ["schedule", "budget", "web", "apps", "learning"] as const;

export type SplittableControl = (typeof SPLITTABLE_CONTROLS)[number];

/** A device's divergence from the base charter — only the controls it splits. */
export type PolicyOverride = Partial<Pick<Policy, SplittableControl>>;

/**
 * What a specific device should actually be sent: the base charter with any of
 * its own overrides laid over the top.
 *
 * Strips `deviceOverrides` unconditionally. If the map reached the wire a
 * device would receive its siblings' rules — a privacy leak, and a warden that
 * could enforce the wrong charter.
 */
export function effectivePolicyForDevice(policy: Policy, deviceId: string): Policy {
  const { deviceOverrides, ...bare } = policy;
  const mine = deviceOverrides?.[deviceId];
  if (!mine) return bare;
  const merged: Policy = { ...bare };
  for (const control of SPLITTABLE_CONTROLS) {
    const value = mine[control];
    if (value !== undefined) {
      // Index-free assignment keeps each control's own type intact.
      Object.assign(merged, { [control]: value });
    }
  }
  return merged;
}

/** Is this control set separately for any device? */
export function isControlSplit(policy: Policy, control: SplittableControl): boolean {
  const overrides = policy.deviceOverrides;
  if (!overrides) return false;
  return Object.values(overrides).some((o) => o[control] !== undefined);
}

/** Every device that currently carries its own value for this control. */
export function splitDeviceIds(policy: Policy, control: SplittableControl): string[] {
  const overrides = policy.deviceOverrides ?? {};
  return Object.keys(overrides).filter((id) => overrides[id][control] !== undefined);
}

/**
 * Start splitting a control: seed EVERY device with what it already had, so
 * splitting changes nothing until the parent edits one. A split that silently
 * altered a child's rules the moment it was switched on would be exactly the
 * kind of surprise this product cannot afford.
 */
export function setControlSplit(
  policy: Policy,
  control: SplittableControl,
  deviceIds: string[],
): Policy {
  const overrides: Record<string, PolicyOverride> = { ...(policy.deviceOverrides ?? {}) };
  for (const id of deviceIds) {
    const current = overrides[id]?.[control] ?? policy[control];
    overrides[id] = { ...overrides[id], [control]: structuredClone(current) };
  }
  return { ...policy, deviceOverrides: overrides };
}

/** Go back to one shared value for this control — the base charter wins. */
export function setControlShared(policy: Policy, control: SplittableControl): Policy {
  const overrides = policy.deviceOverrides;
  if (!overrides) return policy;
  const next: Record<string, PolicyOverride> = {};
  for (const [id, o] of Object.entries(overrides)) {
    const rest: PolicyOverride = { ...o };
    delete rest[control];
    if (Object.keys(rest).length > 0) next[id] = rest;
  }
  // Nothing split anywhere: drop the map, so a child who never used this is
  // indistinguishable from one from before the feature existed.
  if (Object.keys(next).length === 0) {
    const bare: Policy = { ...policy };
    delete bare.deviceOverrides;
    return bare;
  }
  return { ...policy, deviceOverrides: next };
}

/** Edit one device's value for a split control. */
export function setDeviceControl<K extends SplittableControl>(
  policy: Policy,
  deviceId: string,
  control: K,
  value: Policy[K],
): Policy {
  const overrides: Record<string, PolicyOverride> = { ...(policy.deviceOverrides ?? {}) };
  overrides[deviceId] = { ...overrides[deviceId], [control]: value };
  return { ...policy, deviceOverrides: overrides };
}

/**
 * Narrow an override map to just these controls.
 *
 * A save signs ONLY the dimensions the parent actually changed. If the full
 * override map rode along, `effectivePolicyForDevice` would graft an
 * overridden control back onto that policy and emit a clause for a dimension
 * nobody touched — re-signing a stale value with a fresh issuedAt, which the
 * device would then treat as superseding the real one. That is the
 * dual-dimension save-revert bug wearing a different hat.
 */
export function pickOverrides(
  overrides: Record<string, PolicyOverride> | undefined,
  controls: readonly SplittableControl[],
): Record<string, PolicyOverride> | undefined {
  if (!overrides) return undefined;
  const out: Record<string, PolicyOverride> = {};
  for (const [deviceId, o] of Object.entries(overrides)) {
    const kept: PolicyOverride = {};
    for (const c of controls) {
      if (o[c] !== undefined) Object.assign(kept, { [c]: o[c] });
    }
    if (Object.keys(kept).length > 0) out[deviceId] = kept;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

/** What a device currently uses for a control (its override, else the base). */
export function deviceControlValue<K extends SplittableControl>(
  policy: Policy,
  deviceId: string,
  control: K,
): Policy[K] {
  return (policy.deviceOverrides?.[deviceId]?.[control] as Policy[K]) ?? policy[control];
}
