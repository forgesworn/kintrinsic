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
import type { StatusGuardians } from "./statusGuardians";

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
  /**
   * When the move stops being pending if not confirmed (unix s): its
   * `issuedAt` plus 7 days. Absent on a move recorded before this field.
   */
  expiresAt?: number;
  /** Set (unix s) when this install cancelled the move. */
  cancelledAt?: number;
}

/** A statement this install sent, by its `d`: what 3b shows and cancels. */
export interface SentStatement {
  id: string;
  type: StatementType;
  issuedAt: number;
  /** The key the statement names (W or R2). */
  key: string;
}

/**
 * A ward this install confirmed in person at (its machine key came from the
 * presence code) before it knew the device from a pairing or a roster: a
 * joining parent. Its STATUS is admitted and its guardians view kept here,
 * for display only, until a roster from an anchored root names the device or
 * the entry expires. It vouches for nothing: anyone can make a presence code
 * name any machine. See docs/guardian-keys-open-questions.md, "A joining
 * parent's devices" and "Anchoring a joining parent's trust".
 */
export interface PresenceWard {
  /** When this install confirmed in person there (unix s). */
  confirmedAt: number;
  /** The newest STATUS read from it: its `ts`, version and guardians. */
  ts?: number;
  appVersionCode?: number;
  guardians?: StatusGuardians;
}

/** A root this install was told to trust, in person (a scanned guardian code) or by inviting it. */
export interface AnchoredRoot {
  /** When it was anchored (unix s). */
  anchoredAt: number;
  how: "scanned" | "invited";
  /** What this install calls them (the invitation's label), if anything. */
  label?: string;
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
  /** Wards confirmed in person here that no child of this install has yet, by machine. */
  presenceWards: Record<string, PresenceWard>;
  /**
   * Other parents' roots this install trusts, by root (64 hex): the one whose
   * guardian code a joining parent scanned, and any this install invited.
   * With this install's own root, the only roots whose working keys may send
   * it a roster or receive one from it. A ward's STATUS never adds to it:
   * any machine key can claim any roots. docs/guardian-keys-open-questions.md,
   * "Anchoring a joining parent's trust".
   */
  anchoredRoots: Record<string, AnchoredRoot>;
  /**
   * The root this install last used, kept when My Signet is disconnected
   * (`root` is cleared then): the key devices paired under it still pin, and
   * the only identity that may approve this phone again while no device
   * reports its guardians.
   */
  forgottenRoot?: string;
}

export function emptyGuardianKeys(): GuardianKeysState {
  return { v: 1, approvalSentAt: {}, pinSentAt: {}, roster: { children: [] }, sent: {}, presenceWards: {}, anchoredRoots: {} };
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
    presenceWards: rec(o.presenceWards) as Record<string, PresenceWard>,
    anchoredRoots: Object.fromEntries(
      Object.entries(rec(o.anchoredRoots) as Record<string, AnchoredRoot>).filter(
        ([k, v]) => /^[0-9a-f]{64}$/.test(k) && typeof v === "object" && v !== null,
      ),
    ),
  };
}
