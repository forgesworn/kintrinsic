// The presence code a ward shows when a guardian asks to confirm a pending
// co-guardian, removal or move in person. Build-defined (contract leaves the
// encoding open; docs/guardian-keys-open-questions.md, "Presence code
// encoding"), mirroring the scan-to-pair invite:
//
//   charter://presence?m=<machine 64hex>&t=<token 32hex>&d=<pending d, percent-encoded>&s=<statement event id 64hex>
//
// The short form a guardian can type is the token alone, 32 hex shown as four
// groups of 8; the app then asks which device and which pending statement.

import { splitPendingD, type PendingType } from "./sdk";

export interface PresenceCode {
  machine: string;
  token: string;
  /** The pending statement's `d`. */
  d: string;
  /** What the pending statement is, from its `d`. */
  kind: PendingType;
  /** The key the pending statement names (R2), from its `d`. */
  key: string;
  /** The pending statement's event id (the confirmation's `statement`). */
  statement: string;
}

const PREFIX = "charter://presence?";
const HEX64 = /^[0-9a-f]{64}$/;
const HEX32 = /^[0-9a-f]{32}$/;

/** Parse a scanned presence code; null for anything malformed. */
export function parsePresenceCode(code: string): PresenceCode | null {
  const c = code.trim();
  if (!c.startsWith(PREFIX)) return null;
  let q: URLSearchParams;
  try {
    q = new URLSearchParams(c.slice(PREFIX.length));
  } catch {
    return null;
  }
  // Exactly one of each: a repeated parameter is ambiguous, so refused.
  for (const k of ["m", "t", "d", "s"]) if (q.getAll(k).length !== 1) return null;
  const machine = q.get("m") as string;
  const token = q.get("t") as string;
  const d = q.get("d") as string;
  const statement = q.get("s") as string;
  // Lowercase hex only, as the ward writes it: no case folding, so a code
  // that was altered on the way is refused rather than repaired.
  if (!HEX64.test(machine) || !HEX32.test(token) || !HEX64.test(statement)) return null;
  const target = splitPendingD(d);
  if (!target) return null;
  return { machine, token, d, kind: target.type, key: target.key, statement };
}

/**
 * Parse the typed short form: the 32-hex token, optionally in groups
 * separated by spaces or dashes. Case-insensitive, since a person types it.
 */
export function parsePresenceToken(typed: string): string | null {
  const t = typed.replace(/[\s-]+/g, "").toLowerCase();
  return HEX32.test(t) ? t : null;
}
