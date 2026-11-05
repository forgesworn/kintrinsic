// What this guardian install persists about *Guardian roots and working keys*.
// It lives inside the household state (`CharterState.guardianKeys`), so the
// family backup carries it with everything else.
//
// - W, the working key, is the install's existing local guardian key
//   (`signer/guardianKey.ts`), or, for a family whose root is this phone's own
//   key, a separate key minted for it (`signer/phoneWorkingKey.ts`, decision
//   D5). Neither secret is stored here.
// - `root` is set once R has approved W: My Signet (or a Heartwood behind My
//   Signet) connected as this install's root, or the phone's own key (`via:
//   "phone"`) once it has minted W. With no `root`, the install is a
//   local-only family that hasn't needed W yet: the local key IS the pinned
//   root and signs directly, as before the section.

import type { GuardianPinRecord, LocalRoster, SignedEvent, StatementType } from "./sdk";
import type { StatusGuardians } from "./statusGuardians";

/**
 * How the root is held: "signet" and "heartwood" are reached through My
 * Signet's NIP-46 bunker; "phone" is this phone's own guardian key, which
 * signs without a prompt.
 */
export type RootVia = "signet" | "heartwood" | "phone";

export interface InstallRoot {
  /** R, 64 lowercase hex: the bunker's pubkey, or the phone's own guardian key. */
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

/** A roster waiting for a self-vouched ward's STATUS. */
export interface HeldRoster {
  sealAuthor: string;
  inner: SignedEvent;
  /** When it was held (unix s). */
  heldAt: number;
}

/**
 * The guardian PIN on one Android ward, as this install sent it (decision D4):
 * "sending" from the send until a STATUS from that ward shows `pinSet`, then
 * "set". The PIN itself is never kept.
 */
export interface PinDelivery {
  state: "sending" | "set";
  /**
   * While "sending": the record (salt and scrypt hash) that was sent, resent
   * unchanged on each STATUS that doesn't yet show `pinSet`. Dropped once set.
   */
  record?: GuardianPinRecord;
  /** `created_at` of the last RELAYS that carried it (unix s). */
  sentAt: number;
  /** `ts` of the last STATUS from the ward acted on, so one STATUS triggers one resend at most. */
  statusTs?: number;
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
  /** Per Android ward machine: the guardian PIN this install sent, until the ward says it holds one. */
  pins: Record<string, PinDelivery>;
  /**
   * Per ward machine: the pending removals of this install's root it has
   * objected to there (decision Q1), by the removal's event id, or its `d`
   * from a ward that doesn't report ids. Kept while the ward still lists them.
   */
  objected: Record<string, string[]>;
  /**
   * Per ward machine: the `ts` of the newest STATUS this install has replied
   * to (`noteWardStatus`). STATUS is polled, so the same or an older one
   * arrives again; only a newer one is acted on.
   */
  statusRepliedTs: Record<string, number>;
  /**
   * Per ward machine: its STATUS `clamped` counter (events dated ahead of its
   * clock, taken at its own time instead, decision D1) as last read, and when
   * it last rose between two STATUS reports (unix s, this phone's clock).
   */
  clamped: Record<string, { count: number; roseAt?: number }>;
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
   * Machines this install confirmed in person at (the key came from a code it
   * scanned at the device), by machine: when. With the devices it paired
   * itself, the only wards whose STATUS may vouch for a roster's sender.
   */
  presenceConfirmed: Record<string, number>;
  /**
   * Rosters from a trusted root's working key that arrived before any ward
   * this install vouches for itself had reported, by sender (newest kept, a
   * few at most). Re-checked when such a ward reports; dropped after a week.
   */
  heldRosters: Record<string, HeldRoster>;
  /**
   * The root this install last used, kept when My Signet is disconnected
   * (`root` is cleared then): the key devices paired under it still pin, and
   * the only identity that may approve this phone again while no device
   * reports its guardians.
   */
  forgottenRoot?: string;
  /**
   * Set when this household was restored from a backup (decision D5): when,
   * and the working keys (public keys and labels) this identity had then. The
   * lost phone's key is offered for withdrawal while the devices still list
   * it; cleared when the guardian is done.
   */
  restoredFrom?: { at: number; workingKeys: { pubkey: string; label?: string }[] };
}

export function emptyGuardianKeys(): GuardianKeysState {
  return { v: 1, approvalSentAt: {}, pins: {}, objected: {}, statusRepliedTs: {}, clamped: {}, roster: { children: [] }, sent: {}, presenceWards: {}, anchoredRoots: {}, presenceConfirmed: {}, heldRosters: {} };
}

function reviveRestored(x: unknown): GuardianKeysState["restoredFrom"] {
  if (typeof x !== "object" || x === null) return undefined;
  const o = x as { at?: unknown; workingKeys?: unknown };
  if (typeof o.at !== "number" || !Array.isArray(o.workingKeys)) return undefined;
  const workingKeys = o.workingKeys.flatMap((k) => {
    const e = k as { pubkey?: unknown; label?: unknown } | null;
    if (!e || typeof e.pubkey !== "string" || !/^[0-9a-f]{64}$/.test(e.pubkey)) return [];
    return [{ pubkey: e.pubkey, ...(typeof e.label === "string" ? { label: e.label } : {}) }];
  });
  return { at: o.at, workingKeys };
}

/** Revive a persisted value; anything unusable becomes the empty state. */
export function reviveGuardianKeys(x: unknown): GuardianKeysState {
  const empty = emptyGuardianKeys();
  if (typeof x !== "object" || x === null || Array.isArray(x)) return empty;
  // `pinSentAt` was the PIN record before D4: dropped, since STATUS `pinSet` now says.
  const { pinSentAt: _legacy, ...o } = x as Partial<GuardianKeysState> & { pinSentAt?: unknown };
  void _legacy;
  if (o.v !== 1) return empty;
  const rec = (r: unknown) => (typeof r === "object" && r !== null && !Array.isArray(r) ? r : {});
  return {
    ...empty,
    ...o,
    approvalSentAt: rec(o.approvalSentAt) as Record<string, number>,
    pins: Object.fromEntries(
      Object.entries(rec(o.pins) as Record<string, PinDelivery>).filter(
        ([, v]) => typeof v === "object" && v !== null && (v.state === "sending" || v.state === "set") && typeof v.sentAt === "number",
      ),
    ),
    objected: Object.fromEntries(
      Object.entries(rec(o.objected) as Record<string, unknown>).filter(
        (e): e is [string, string[]] => Array.isArray(e[1]) && e[1].every((x) => typeof x === "string"),
      ),
    ),
    statusRepliedTs: Object.fromEntries(
      Object.entries(rec(o.statusRepliedTs) as Record<string, unknown>).filter(
        (e): e is [string, number] => typeof e[1] === "number" && Number.isFinite(e[1]),
      ),
    ),
    clamped: Object.fromEntries(
      Object.entries(rec(o.clamped) as Record<string, unknown>).filter(
        (e): e is [string, { count: number; roseAt?: number }] =>
          typeof e[1] === "object" &&
          e[1] !== null &&
          typeof (e[1] as { count?: unknown }).count === "number" &&
          ((e[1] as { roseAt?: unknown }).roseAt === undefined || typeof (e[1] as { roseAt?: unknown }).roseAt === "number"),
      ),
    ),
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
    presenceConfirmed: rec(o.presenceConfirmed) as Record<string, number>,
    heldRosters: rec(o.heldRosters) as Record<string, HeldRoster>,
    restoredFrom: reviveRestored(o.restoredFrom),
  };
}

/** This install's root when it is reached through My Signet (a bunker that prompts), not the phone's own key. */
export function bunkerRoot(k: Pick<GuardianKeysState, "root"> | undefined): InstallRoot | undefined {
  return k?.root && k.root.via !== "phone" ? k.root : undefined;
}

/** Is this family's guardian identity this phone's own key (with or without a minted working key)? */
export function phoneHeldRoot(k: Pick<GuardianKeysState, "root"> | undefined): boolean {
  return !k?.root || k.root.via === "phone";
}
