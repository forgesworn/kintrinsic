// What this guardian install persists about *Guardian roots and working keys*.
// It lives inside the household state (`CharterState.guardianKeys`), so the
// family backup carries it with everything else.
//
// - W, the working key, is the install's existing local guardian key
//   (`signer/guardianKey.ts`); it is not stored here.
// - `root` is set once My Signet (or a Heartwood behind My Signet) is
//   connected as this install's root R and R has approved W. With no `root`,
//   the install is a local-only family: the local key IS the pinned root and
//   signs directly, as before the section.

import type { LocalRoster, SignedEvent, StatementType } from "./sdk";

/** How the root is held. Both are reached through My Signet's NIP-46 bunker. */
export type RootVia = "signet" | "heartwood";

export interface InstallRoot {
  /** R, 64 lowercase hex: the bunker's pubkey. */
  pubkey: string;
  /** R's signed approval of this install's working key (kind 31120). */
  approval: SignedEvent;
  via: RootVia;
}

/** A root move this install started: R2, R2's approval of W, and the move. */
export interface NextRoot extends InstallRoot {
  /** The `bunker://` link R2 is reached by. Becomes the signer's once the move lands. */
  bunkerUri: string;
  /** The move statement's `d` and event id. */
  moveD: string;
  moveId: string;
}

/** A statement this install sent, by its `d`: what 3b shows and cancels. */
export interface SentStatement {
  id: string;
  type: StatementType;
  issuedAt: number;
  /** The key the statement names (W or R2). */
  key: string;
}

export interface GuardianKeysState {
  v: 1;
  /** This install's root and its approval of W; absent for a local-only family. */
  root?: InstallRoot;
  /** A root move in progress. */
  nextRoot?: NextRoot;
  /** The label W's approval carries (≤ 64 UTF-8 bytes). */
  installLabel?: string;
  /** Per ward machine: when W's approval was last sent there (unix s). */
  approvalSentAt: Record<string, number>;
  /** Per Android ward machine: when a guardian PIN was last sent (unix s). The PIN itself is never kept. */
  pinSentAt: Record<string, number>;
  /** The family roster as merged from other guardians' installs (subject = `Child.rosterId`). */
  roster: LocalRoster;
  /** Statements this install sent, by `d`. */
  sent: Record<string, SentStatement>;
}

export function emptyGuardianKeys(): GuardianKeysState {
  return { v: 1, approvalSentAt: {}, pinSentAt: {}, roster: { children: [] }, sent: {} };
}

/** Revive a persisted value; anything unusable becomes the empty state. */
export function reviveGuardianKeys(x: unknown): GuardianKeysState {
  const empty = emptyGuardianKeys();
  if (typeof x !== "object" || x === null || Array.isArray(x)) return empty;
  const o = x as Partial<GuardianKeysState>;
  if (o.v !== 1) return empty;
  const rec = (r: unknown) => (typeof r === "object" && r !== null && !Array.isArray(r) ? r : {});
  return {
    ...empty,
    ...o,
    approvalSentAt: rec(o.approvalSentAt) as Record<string, number>,
    pinSentAt: rec(o.pinSentAt) as Record<string, number>,
    roster:
      typeof o.roster === "object" && o.roster !== null && Array.isArray(o.roster.children)
        ? o.roster
        : empty.roster,
    sent: rec(o.sent) as Record<string, SentStatement>,
  };
}
