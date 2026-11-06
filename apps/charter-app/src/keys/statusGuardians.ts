// STATUS `guardians` (contract, *Guardian roots and working keys*, STATUS):
// who the ward trusts, as it sees it.
//
//   { roots: [{ root, keys: [W…] }], pending: [{ d, id?, signer?, kind, takesEffectAt?, expiresAt }],
//     tombstones, relays, pinSet?, pinSetAt?, clamped? }
//
// Parsed strictly: anything malformed drops the WHOLE object (undefined), so a
// half-read view never says a key is or isn't trusted when it can't be sure.
// Absent means a ward that predates the section. `pending[].id`,
// `pending[].signer`, `pinSet`, `pinSetAt` and `clamped` are newer than the rest, so a ward that doesn't send them yet
// still parses; each is checked when present.

import { splitPendingD, type PendingType, type StatusGuardians as RefStatusGuardians } from "./sdk";

// The newer fields are optional here whether or not the reference has them
// yet, so both shapes compile: a ward that predates them still parses.
/**
 * One pending statement a ward reports; `id` is its event id and `signer` the
 * 64-hex key that signed it (newer wards).
 */
export type StatusPending = Omit<RefStatusGuardians["pending"][number], "id" | "signer"> & { id?: string; signer?: string };

/** STATUS `guardians` as this app reads it: the reference's shape plus the newer fields. */
export interface StatusGuardians extends Omit<RefStatusGuardians, "pending" | "pinSet" | "pinSetAt" | "clamped"> {
  pending: StatusPending[];
  /** Android: whether the ward holds a guardian PIN. Absent = the ward doesn't say. */
  pinSet?: boolean;
  /**
   * Android: the `created_at` of the RELAYS that set the PIN it holds, so an
   * install can tell its own PIN from an older one or another install's.
   * Absent = no PIN, or a ward that predates it.
   */
  pinSetAt?: number;
  /** Events whose claimed time was ahead of the ward's clock and was clamped to it. */
  clamped?: number;
}

const HEX64 = /^[0-9a-f]{64}$/;
const KINDS: readonly PendingType[] = ["coguardian", "removal", "move"];

function isUint(x: unknown): x is number {
  return typeof x === "number" && Number.isSafeInteger(x) && x >= 0;
}

/** Parse a STATUS payload's `guardians` field; undefined when absent or malformed. */
export function parseStatusGuardians(
  x: unknown,
  opts: { relaysOptional?: boolean } = {},
): StatusGuardians | undefined {
  if (typeof x !== "object" || x === null || Array.isArray(x)) return undefined;
  const o = { ...(x as Record<string, unknown>) };
  // A departed key's last STATUS carries no relay list (OQ "A departed key's
  // last STATUS"); everywhere else it is required.
  if (opts.relaysOptional && o.relays === undefined) o.relays = [];
  if (!Array.isArray(o.roots) || !Array.isArray(o.pending) || !Array.isArray(o.relays)) return undefined;
  if (!isUint(o.tombstones)) return undefined;
  // The contract caps roots at 4 and keys at 16 per root; a little headroom
  // tolerates nothing hostile but a bigger list is not a real ward's.
  if (o.roots.length > 8 || o.pending.length > 64 || o.relays.length > 32) return undefined;
  const roots: StatusGuardians["roots"] = [];
  for (const r of o.roots) {
    if (typeof r !== "object" || r === null) return undefined;
    const e = r as Record<string, unknown>;
    if (typeof e.root !== "string" || !HEX64.test(e.root)) return undefined;
    if (!Array.isArray(e.keys) || e.keys.length > 64) return undefined;
    if (!e.keys.every((k): k is string => typeof k === "string" && HEX64.test(k))) return undefined;
    roots.push({ root: e.root, keys: [...e.keys] });
  }
  const pending: StatusGuardians["pending"] = [];
  for (const p of o.pending) {
    if (typeof p !== "object" || p === null) return undefined;
    const e = p as Record<string, unknown>;
    if (typeof e.d !== "string" || !KINDS.includes(e.kind as PendingType)) return undefined;
    const split = splitPendingD(e.d);
    if (!split || split.type !== e.kind) return undefined;
    if (!isUint(e.expiresAt)) return undefined;
    if (e.takesEffectAt !== undefined && !isUint(e.takesEffectAt)) return undefined;
    if (e.id !== undefined && (typeof e.id !== "string" || !HEX64.test(e.id))) return undefined;
    if (e.signer !== undefined && (typeof e.signer !== "string" || !HEX64.test(e.signer))) return undefined;
    pending.push({
      d: e.d,
      ...(e.id !== undefined ? { id: e.id as string } : {}),
      ...(e.signer !== undefined ? { signer: e.signer as string } : {}),
      kind: e.kind as PendingType,
      ...(e.takesEffectAt !== undefined ? { takesEffectAt: e.takesEffectAt as number } : {}),
      expiresAt: e.expiresAt,
    });
  }
  if (!o.relays.every((r): r is string => typeof r === "string" && r.length > 0 && r.length <= 512)) {
    return undefined;
  }
  if (o.pinSet !== undefined && typeof o.pinSet !== "boolean") return undefined;
  if (o.pinSetAt !== undefined && !isUint(o.pinSetAt)) return undefined;
  if (o.clamped !== undefined && !isUint(o.clamped)) return undefined;
  return {
    roots,
    pending,
    tombstones: o.tombstones,
    relays: [...o.relays],
    ...(o.pinSet !== undefined ? { pinSet: o.pinSet } : {}),
    ...(o.pinSetAt !== undefined ? { pinSetAt: o.pinSetAt } : {}),
    ...(o.clamped !== undefined ? { clamped: o.clamped } : {}),
  };
}

/** Where a key stands on one ward, from its STATUS `guardians`. */
export type KeyStanding =
  /** The ward pins this key as a root. */
  | { as: "root" }
  /** The ward holds this key as a working key bound to `root`. */
  | { as: "working"; root: string }
  | { as: "none" };

export function keyStanding(g: StatusGuardians, key: string): KeyStanding {
  if (g.roots.some((r) => r.root === key)) return { as: "root" };
  const bound = g.roots.find((r) => r.keys.includes(key));
  return bound ? { as: "working", root: bound.root } : { as: "none" };
}
