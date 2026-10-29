// STATUS `guardians` (contract, *Guardian roots and working keys*, STATUS):
// who the ward trusts, as it sees it.
//
//   { roots: [{ root, keys: [W…] }], pending: [{ d, kind, takesEffectAt?, expiresAt }],
//     tombstones, relays }
//
// Parsed strictly: anything malformed drops the WHOLE object (undefined), so a
// half-read view never says a key is or isn't trusted when it can't be sure.
// Absent means a ward that predates the section.

import { splitPendingD, type PendingType, type StatusGuardians } from "./sdk";

export type { StatusGuardians };

const HEX64 = /^[0-9a-f]{64}$/;
const KINDS: readonly PendingType[] = ["coguardian", "removal", "move"];

function isUint(x: unknown): x is number {
  return typeof x === "number" && Number.isSafeInteger(x) && x >= 0;
}

/** Parse a STATUS payload's `guardians` field; undefined when absent or malformed. */
export function parseStatusGuardians(x: unknown): StatusGuardians | undefined {
  if (typeof x !== "object" || x === null || Array.isArray(x)) return undefined;
  const o = x as Record<string, unknown>;
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
    pending.push({
      d: e.d,
      kind: e.kind as PendingType,
      ...(e.takesEffectAt !== undefined ? { takesEffectAt: e.takesEffectAt as number } : {}),
      expiresAt: e.expiresAt,
    });
  }
  if (!o.relays.every((r): r is string => typeof r === "string" && r.length > 0 && r.length <= 512)) {
    return undefined;
  }
  return { roots, pending, tombstones: o.tombstones, relays: [...o.relays] };
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
