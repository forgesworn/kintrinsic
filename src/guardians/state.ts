/**
 * The ward side of *Guardian roots and working keys (v1)* as a pure state
 * machine.
 *
 * - The state is plain JSON (no Maps, Sets or class instances) so it can be
 *   persisted as is and compared byte-for-byte in the golden vectors.
 * - Nothing here reads a clock or draws randomness: every entry point takes
 *   `now` (Unix seconds) and the ward mints presence tokens itself.
 * - Every entry point first advances timers to `now` (pending expiry, the
 *   72-hour removal notice, token lifetime), exactly as `tick` does, so a
 *   departure that is due is applied before anything else is judged.
 * - Every entry point returns a new state; the input is never mutated.
 *
 * Where the contract is silent this module takes the fail-closed reading and
 * says so with a pointer into docs/guardian-keys-open-questions.md ("OQ: …").
 */
import type { ReasonCode, TickResult } from './reasons.js'
import { tryParseStrictJson } from './json.js'
import { validateGuardianPinRecord, type GuardianPinRecord } from './pin.js'
import {
  isEventShape,
  verifySignedEvent,
  hasMarker,
  parseStatement,
  STATEMENT_PREFIXES,
  type PendingType,
  type Statement,
} from './statements.js'
import {
  CHARTER_DEVICE_PAIR_OFFER,
  CHARTER_PRESENCE_CONFIRM,
  HEX32,
  MAX_ACTIVE_KEYS_PER_ROOT,
  MAX_HELD_PER_ROOT,
  MAX_ROOTS,
  MAX_TOMBSTONES_PER_ROOT,
  OBJECTION_BLOCK_SECONDS,
  PENDING_MAX_SECONDS,
  PREVIOUS_RELAYS_MAX,
  PREVIOUS_RELAYS_SECONDS,
  RELAYS_TTL_SECONDS,
  REMOVAL_NOTICE_SECONDS,
  TOKEN_LIFETIME_SECONDS,
  constantTimeEqual,
  isHex64,
  isUnixSeconds,
  type Inbound,
  type SignedEvent,
} from './types.js'

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

export interface PinnedRoot {
  root: string
  pinnedAt: number
  via: 'pair' | 'coguardian' | 'move'
}

export interface WorkingKeyRecord {
  key: string
  /** The binding root (rule 1). For a departed key, the root it was bound to. */
  root: string
  status: 'active' | 'withdrawn' | 'departed'
  label: string | null
  approvalId: string
  approvalIssuedAt: number
  /** Why the key left the signer set, if it did. */
  departedBy: 'withdrawal' | 'removal' | 'move' | null
  /** For a key departed by a root move: the new root that may re-bind it. */
  rebindRoot: string | null
}

/**
 * The newest accepted statement per slot (rule 2). The slot is the `d`,
 * except a cancel (`d` + signer) and an objection (`d` + `p`), so one
 * signer's cancel or one objection never displaces another's. Withdrawals
 * live in `heldWithdrawals` / `tombstones`.
 */
export interface StoredStatement {
  slot: string
  d: string
  id: string
  type: Exclude<Statement['type'], 'withdrawal'>
  signer: string
  p: string
  issuedAt: number
  label: string | null
  expiresAt: number | null
  /** The ward clock when it was accepted. */
  receivedAt: number
  /** A cancel or objection that had nothing to act on when it arrived. */
  held: boolean
  /** The seal's author: a held statement goes when its signer or its sealer departs. */
  seal: string
}

export interface PendingRecord {
  d: string
  type: PendingType
  /** The pending statement's event id (named by a presence confirmation). */
  id: string
  signer: string
  /** R2. */
  key: string
  issuedAt: number
  /** The ward clock when the statement was accepted. */
  receivedAt: number
  expiresAt: number
  confirmedAt: number | null
  takesEffectAt: number | null
}

export interface HeldWithdrawal {
  d: string
  id: string
  signer: string
  key: string
  issuedAt: number
}

export interface Tombstone {
  type: 'withdrawal' | 'removal'
  d: string
  id: string
  key: string
  signer: string
  issuedAt: number
}

export interface DepartedRoot {
  root: string
  issuedAt: number
  by: 'removal' | 'move'
}

export interface PresenceToken {
  token: string
  /** The pending statement the token was minted for, or null for any. */
  statementD: string | null
  mintedAt: number
}

/**
 * The highest real `issuedAt` a ward has accepted from one signer in one
 * slot. A signer's `issuedAt` only ever rises (*Who may sign what*), so an
 * event at or below it is a replay.
 */
export interface SignerFloor {
  signer: string
  issuedAt: number
}

/**
 * A statement slot's floor for one seal author (the slot as rule 2 defines
 * it): the highest real `issuedAt` accepted under that key's seal. Keyed by
 * the install that sealed it, not the signer, since a root signs for each of
 * its installs at that install's clock (seal exception 2).
 */
export interface StatementFloor {
  slot: string
  seal: string
  issuedAt: number
}

/** A held clause: the minimal model of rules 4 and 5. */
export interface HeldClause {
  subject: string
  kind: string
  signer: string
  /** The signer's root at acceptance, for "set by" display (rule 5). */
  root: string
  /** The clause's event id. */
  id: string
  /** The clause's own `issuedAt`, as signed. */
  issuedAt: number
  /**
   * The slot's bar: a replacement must have `issuedAt` above it. The clause's
   * effective time, min(`issuedAt`, the ward clock at acceptance), never
   * below the bar it replaced; when the signer departs, min(that, the
   * departure time).
   * OQ: Clause floor after a departure.
   */
  floor: number
  /**
   * Each signer's floor for this `(subject, kind)`: the highest `issuedAt`
   * accepted from it here. Sorted by signer. A signer that can never sign
   * again (a withdrawn key, or a working key of a removed root) is dropped.
   * The held clause also counts for its own signer (`clauseFloor`).
   */
  signerFloors: SignerFloor[]
}

export interface GuardianState {
  v: 1
  pairedAt: number
  roots: PinnedRoot[]
  keys: WorkingKeyRecord[]
  statements: StoredStatement[]
  /**
   * Each seal author's floor per statement slot, sorted by slot then seal
   * author. The stored statement also counts for its own seal author
   * (`statementFloor`).
   */
  statementFloors: StatementFloor[]
  pending: PendingRecord[]
  heldWithdrawals: HeldWithdrawal[]
  tombstones: Tombstone[]
  departedRoots: DepartedRoot[]
  tokens: PresenceToken[]
  clauses: HeldClause[]
  relays: string[]
  relaysUpdatedAt: number
  /** Relay lists replaced within the last 7 days, oldest first (rule 11). */
  previousRelays: { relays: string[]; until: number }[]
  /** The Android guardian PIN record, set by a gift-wrapped RELAYS. */
  guardianPin: GuardianPinRecord | null
  /**
   * Each signer's PIN replay floor: the highest `created_at` of a RELAYS
   * carrying a PIN accepted from it. Sorted by signer. Separate from
   * `relaysUpdatedAt`, so a PIN never makes a later relay code look used.
   */
  pinFloors: SignerFloor[]
  /**
   * The PIN's bar, across signers: the effective time, `min(created_at, the
   * ward clock)`, of the PIN in force, never falling. A RELAYS carrying a PIN
   * is taken only above it, so an older PIN from another install processed
   * later can't roll back a newer one (rule 11). 0 before any PIN. Absent
   * in a state saved before this rule: `loadState` seeds it.
   */
  pinBar: number
  /** The `created_at` of the RELAYS that set the PIN in force; null before any. STATUS `guardians.pinSetAt`. */
  pinSetAt: number | null
  /** Departed keys owed one last STATUS showing their departure. */
  farewell: string[]
  /**
   * Accepted clauses and statements whose `issuedAt` was ahead of the ward
   * clock (and so clamped to it), since pairing. STATUS `guardians.clamped`.
   */
  clamped: number
}

export interface TickEntry {
  d: string
  result: TickResult
  reason: ReasonCode
}

export interface StepOutcome {
  result: 'accepted' | 'dropped'
  reason: ReasonCode
  /** Pending statements resolved by the timer advance that ran first. */
  ticked: TickEntry[]
  /** Keys that left the signer set during this step. */
  departed: string[]
}

export interface StepResult {
  state: GuardianState
  outcome: StepOutcome
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0
}

function clone(s: GuardianState): GuardianState {
  return loadState(JSON.parse(JSON.stringify(s)) as GuardianState)
}

/**
 * A saved state, with the members a state saved before their rule lacks
 * filled in as the Rust reference reads them: no statement or PIN floors,
 * nothing clamped, no `pinSetAt`, and the PIN bar at the saved `pinSetAt`
 * if any, else 0. Every step does this to the state it is given. Changes
 * `saved` in place and returns it.
 */
export function loadState(saved: SavedGuardianState): GuardianState {
  saved.statementFloors ??= []
  saved.pinFloors ??= []
  saved.clamped ??= 0
  saved.pinSetAt ??= null
  saved.pinBar ??= saved.pinSetAt ?? 0
  return saved as GuardianState
}

/** The members a state saved before their rule lacks. */
type LaterMembers = 'statementFloors' | 'pinFloors' | 'clamped' | 'pinSetAt' | 'pinBar'

/** A saved `GuardianState`, which may predate some of its members. */
export type SavedGuardianState = Omit<GuardianState, LaterMembers> & Partial<Pick<GuardianState, LaterMembers>>

/** Sort every list whose order carries no meaning, so JSON is stable. */
function normalise(s: GuardianState): GuardianState {
  s.keys.sort((a, b) => cmp(a.key, b.key))
  s.statements.sort((a, b) => cmp(a.slot, b.slot))
  s.statementFloors.sort((a, b) => cmp(a.slot, b.slot) || cmp(a.seal, b.seal))
  s.pending.sort((a, b) => cmp(a.d, b.d))
  s.heldWithdrawals.sort((a, b) => cmp(a.d, b.d) || cmp(a.signer, b.signer))
  s.departedRoots.sort((a, b) => cmp(a.root, b.root))
  s.clauses.sort((a, b) => cmp(a.subject, b.subject) || cmp(a.kind, b.kind))
  s.farewell = [...new Set(s.farewell)].sort(cmp)
  return s
}

export function isPinned(s: GuardianState, key: string): boolean {
  return s.roots.some((r) => r.root === key)
}

function keyRecord(s: GuardianState, key: string): WorkingKeyRecord | undefined {
  return s.keys.find((k) => k.key === key)
}

function activeKey(s: GuardianState, key: string): WorkingKeyRecord | undefined {
  const k = keyRecord(s, key)
  return k && k.status === 'active' && isPinned(s, k.root) ? k : undefined
}

/** A root that was removed or moved away and is not pinned again. */
function isDepartedRoot(s: GuardianState, key: string): boolean {
  return !isPinned(s, key) && s.departedRoots.some((d) => d.root === key)
}

/** Every pinned root, plus every active working key bound to a pinned root. Sorted. */
export function signerSet(s: GuardianState): string[] {
  const out = s.roots.map((r) => r.root)
  for (const k of s.keys) if (k.status === 'active' && isPinned(s, k.root)) out.push(k.key)
  return [...new Set(out)].sort(cmp)
}

/** The generic signer-set gate for CLAUSE, GRANT, RELAYS and USAGE_SYNC authors. */
export function isSigner(s: GuardianState, pubkey: string): boolean {
  return isPinned(s, pubkey) || activeKey(s, pubkey) !== undefined
}

/** The root a signer speaks for: itself if pinned, its binding root if an active W. */
export function rootOf(s: GuardianState, pubkey: string): string | null {
  if (isPinned(s, pubkey)) return pubkey
  return activeKey(s, pubkey)?.root ?? null
}

/**
 * A key that is not in the signer set and is known as withdrawn, departed or
 * a departed root (rule 4). Includes a key withdrawn the moment it bound.
 */
export function isDeparted(s: GuardianState, key: string): boolean {
  if (isSigner(s, key)) return false
  const k = keyRecord(s, key)
  if (k && k.status !== 'active') return true
  return isDepartedRoot(s, key)
}

function activeCount(s: GuardianState, root: string): number {
  return s.keys.filter((k) => k.status === 'active' && k.root === root).length
}

/**
 * The root a held statement counts against: an objection's objecting root
 * (the root its removal names, whichever of its keys signed it), else the
 * signer.
 */
function heldOwner(st: { type: Statement['type']; d: string; signer: string }): string {
  return st.type === 'objection' ? st.d.slice(-64) : st.signer
}

function heldCount(s: GuardianState, root: string): number {
  return (
    s.heldWithdrawals.filter((h) => h.signer === root).length +
    s.statements.filter((st) => st.held && heldOwner(st) === root).length
  )
}

/** Rule 10: a root at its tombstone or held quota can't approve new working keys. OQ: Held-statement quota. */
function rootAtQuota(s: GuardianState, root: string): boolean {
  return (
    s.tombstones.filter((t) => t.signer === root).length >= MAX_TOMBSTONES_PER_ROOT ||
    heldCount(s, root) >= MAX_HELD_PER_ROOT
  )
}

/**
 * Rule 9: every W in the signer set, plus each pinned root with no W in the
 * set. Sorted.
 */
export function recipients(s: GuardianState): string[] {
  const out: string[] = []
  for (const r of s.roots) {
    const ws = s.keys.filter((k) => k.status === 'active' && k.root === r.root).map((k) => k.key)
    if (ws.length === 0) out.push(r.root)
    else out.push(...ws)
  }
  return [...new Set(out)].sort(cmp)
}

/** STATUS recipients: the live recipients plus departed keys owed one last STATUS. */
export function statusRecipients(s: GuardianState): string[] {
  return [...new Set([...recipients(s), ...s.farewell])].sort(cmp)
}

/**
 * Rule 11: the relays STATUS goes out on: the current list, then (on a
 * multi-root ward) every list replaced within the last 7 days, newest first,
 * without repeats.
 */
export function statusRelays(s: GuardianState, now: number): string[] {
  const out = [...s.relays]
  if (s.roots.length > 1) {
    for (const prev of [...s.previousRelays].reverse()) {
      if (now >= prev.until) continue
      for (const r of prev.relays) if (!out.includes(r)) out.push(r)
    }
  }
  return out
}

/**
 * Pending expiry: `expiresAt` clamped to 7 days after the statement's
 * effective time (`effectiveTime`); 7 days without one.
 */
export function pendingExpiry(effective: number, expiresAt: number | null): number {
  const cap = effective + PENDING_MAX_SECONDS
  return expiresAt === null ? cap : Math.min(expiresAt, cap)
}

/**
 * A claimed time as the ward uses it: never ahead of the ward clock when the
 * event arrived. A clause's or statement's `issuedAt` ahead of the clock is
 * clamped to it, so one signer's fast clock can't pin a slot against another
 * signer, and no clock state refuses a rule outright.
 */
export function effectiveTime(issuedAt: number, receivedAt: number): number {
  return Math.min(issuedAt, receivedAt)
}

/** The storage slot of a statement: see `StoredStatement`. OQ: Cancel or objection before its statement. */
export function statementSlot(st: { type: Statement['type']; d: string; signer: string; p: string }): string {
  if (st.type === 'cancel') return `${st.d}|${st.signer}`
  if (st.type === 'objection') return `${st.d}|${st.p}`
  return st.d
}

function storedFor(s: GuardianState, slot: string): StoredStatement | undefined {
  return s.statements.find((st) => st.slot === slot)
}

/**
 * A key that can never sign again: a withdrawn working key, or one departed
 * with its removed root. Its floors are dropped. A root that departed may be
 * pinned again by a co-guardian, and a working key departed by a move may be
 * re-bound by the new root, so theirs are kept.
 */
function goneForGood(s: GuardianState, key: string): boolean {
  const k = keyRecord(s, key)
  return !!k && (k.status === 'withdrawn' || (k.status === 'departed' && k.departedBy === 'removal'))
}

/** `list` with `signer`'s floor raised to `issuedAt`. Sorted by signer. */
function raiseFloor(list: SignerFloor[], signer: string, issuedAt: number): SignerFloor[] {
  const prev = list.find((f) => f.signer === signer)
  const out = list.filter((f) => f.signer !== signer).map((f) => ({ ...f }))
  out.push({ signer, issuedAt: prev ? Math.max(prev.issuedAt, issuedAt) : issuedAt })
  return out.sort((a, b) => cmp(a.signer, b.signer))
}

/** `signer`'s floor in a clause slot, or null if it has none. */
function clauseFloor(held: HeldClause, signer: string): number | null {
  const own = held.signer === signer ? held.issuedAt : null
  const f = held.signerFloors.find((x) => x.signer === signer)?.issuedAt ?? null
  return f === null ? own : own === null ? f : Math.max(f, own)
}

/** A stored statement's seal author; one stored before seals were recorded reads as sealed by its signer. */
function sealedBy(st: StoredStatement): string {
  return st.seal === '' ? st.signer : st.seal
}

/** `seal`'s floor in a statement slot, or null if it has none. */
function statementFloor(s: GuardianState, slot: string, seal: string): number | null {
  const stored = storedFor(s, slot)
  const own = stored && sealedBy(stored) === seal ? stored.issuedAt : null
  const f = s.statementFloors.find((x) => x.slot === slot && x.seal === seal)?.issuedAt ?? null
  return f === null ? own : own === null ? f : Math.max(f, own)
}

function raiseStatementFloor(s: GuardianState, slot: string, seal: string, issuedAt: number): void {
  const prev = s.statementFloors.find((x) => x.slot === slot && x.seal === seal)
  s.statementFloors = s.statementFloors.filter((x) => !(x.slot === slot && x.seal === seal))
  s.statementFloors.push({ slot, seal, issuedAt: prev ? Math.max(prev.issuedAt, issuedAt) : issuedAt })
}

/** Rule 4: drop the floors of departing keys that can never sign again. */
function forgetFloors(s: GuardianState, keys: string[]): void {
  const gone = new Set(keys.filter((k) => goneForGood(s, k)))
  if (gone.size === 0) return
  for (const c of s.clauses) c.signerFloors = c.signerFloors.filter((f) => !gone.has(f.signer))
  s.statementFloors = s.statementFloors.filter((f) => !gone.has(f.seal))
  s.pinFloors = s.pinFloors.filter((f) => !gone.has(f.signer))
}

function putStatement(s: GuardianState, st: Statement, seal: string, now: number, held = false): void {
  if (st.type === 'withdrawal') return
  const slot = statementSlot(st)
  // The statement it replaces keeps its seal author's floor (it may have
  // been stored before floors were kept).
  const prev = storedFor(s, slot)
  if (prev && !goneForGood(s, sealedBy(prev))) raiseStatementFloor(s, slot, sealedBy(prev), prev.issuedAt)
  raiseStatementFloor(s, slot, seal, st.issuedAt)
  s.statements = s.statements.filter((x) => x.slot !== slot)
  s.statements.push({
    slot,
    d: st.d,
    id: st.id,
    type: st.type,
    signer: st.signer,
    p: st.p,
    issuedAt: st.issuedAt,
    label: st.label,
    expiresAt: st.expiresAt,
    receivedAt: now,
    held,
    seal,
  })
}

/**
 * Rule 2 within one slot: the newest effective time wins. Re-delivery of the
 * stored event is `duplicate`; an older one is `stale`. At the same effective
 * time the lower event id wins, so every ward converges whatever the arrival
 * order; the loser is `tie_not_newer`. Otherwise a statement whose
 * `issuedAt` is at or below its seal author's floor for the slot is `replay`:
 * without it, a relay could re-deliver a superseded far-future statement,
 * whose effective time would beat the slot's. A relay re-delivers the seal
 * too, so keying the floor by the install that sealed it blocks replays
 * while two installs of one root, asking it to sign at their own clocks,
 * never hold each other back. Run before any change to the state, so a
 * statement that fails here leaves no trace.
 * OQ: Same d, same issuedAt, different content.
 */
function orderAgainst(s: GuardianState, st: Statement, seal: string, now: number): ReasonCode | null {
  const slot = statementSlot(st)
  const stored = storedFor(s, slot)
  if (stored) {
    if (stored.id === st.id) return 'duplicate'
    const mine = effectiveTime(st.issuedAt, now)
    const theirs = effectiveTime(stored.issuedAt, stored.receivedAt)
    if (mine < theirs) return 'stale'
    if (mine === theirs && cmp(st.id, stored.id) > 0) return 'tie_not_newer'
  }
  // A same-second sibling of the stored statement under the same seal is
  // left to the tie rule above, so every ward converges on the lower id.
  const sibling = !!stored && sealedBy(stored) === seal && stored.issuedAt === st.issuedAt
  const floor = statementFloor(s, slot, seal)
  if (floor !== null && (st.issuedAt < floor || (st.issuedAt === floor && !sibling))) return 'replay'
  return null
}

function dropPending(s: GuardianState, pred: (p: PendingRecord) => boolean): void {
  const dropped = new Set(s.pending.filter(pred).map((p) => p.d))
  if (dropped.size === 0) return
  s.pending = s.pending.filter((p) => !dropped.has(p.d))
  s.tokens = s.tokens.filter((t) => t.statementD === null || !dropped.has(t.statementD))
}

function upsertDepartedRoot(s: GuardianState, root: string, issuedAt: number, by: 'removal' | 'move'): void {
  s.departedRoots = s.departedRoots.filter((d) => d.root !== root)
  s.departedRoots.push({ root, issuedAt, by })
}

/** Rule 4 floor: clamp the held clauses of keys departing at `at`. OQ: Clause floor after a departure. */
function clampClauses(s: GuardianState, keys: string[], at: number): void {
  for (const c of s.clauses) {
    if (keys.includes(c.signer)) c.floor = Math.min(c.floor, at)
  }
}

/**
 * Rule 4 for statements: when `keys` depart, every held statement one of them
 * signed or sealed goes (it stops counting against any quota), and so does
 * every objection one of them signed, held or applied, with the block on
 * later removals it recorded (rule 7).
 */
function dropDepartedStatements(s: GuardianState, keys: string[]): void {
  s.statements = s.statements.filter((st) => {
    const signed = keys.includes(st.signer)
    const sealed = keys.includes(st.seal)
    return !((st.held && (signed || sealed)) || (st.type === 'objection' && signed))
  })
}

/**
 * Everything a departure at `at` does to what the departing keys left: their
 * held clauses' bars, their floors, their held statements and their objections.
 */
function depart(s: GuardianState, keys: string[], at: number): void {
  clampClauses(s, keys, at)
  forgetFloors(s, keys)
  dropDepartedStatements(s, keys)
}

function withdraw(
  s: GuardianState,
  rec: WorkingKeyRecord,
  w: { d: string; id: string; signer: string; issuedAt: number },
  at: number,
): void {
  rec.status = 'withdrawn'
  rec.departedBy = 'withdrawal'
  rec.rebindRoot = null
  s.tombstones.push({ type: 'withdrawal', d: w.d, id: w.id, key: rec.key, signer: w.signer, issuedAt: w.issuedAt })
  s.heldWithdrawals = s.heldWithdrawals.filter((h) => h.key !== rec.key)
  depart(s, [rec.key], at)
}

/**
 * Apply a root removal (rule 7) at time `at`. Refused unless another,
 * distinct root remains: removal never leaves a ward with no roots. R2 is
 * recorded as departed at the removal's effective time.
 */
function applyRemoval(
  s: GuardianState,
  r: { d: string; id: string; signer: string; key: string; issuedAt: number },
  effective: number,
  at: number,
): 'applied' | 'last_root' {
  const R2 = r.key
  if (!isPinned(s, R2) || s.roots.filter((x) => x.root !== R2).length < 1) return 'last_root'
  s.roots = s.roots.filter((x) => x.root !== R2)
  upsertDepartedRoot(s, R2, effective, 'removal')
  s.tombstones.push({ type: 'removal', d: r.d, id: r.id, key: R2, signer: r.signer, issuedAt: r.issuedAt })
  const gone = [R2]
  for (const k of s.keys) {
    if (k.status === 'active' && k.root === R2) {
      k.status = 'departed'
      k.departedBy = 'removal'
      k.rebindRoot = null
      gone.push(k.key)
    }
  }
  depart(s, gone, at)
  // "Once a removal applies, every pending statement signed by R2 is dropped."
  dropPending(s, (p) => p.signer === R2 || p.key === R2)
  s.heldWithdrawals = s.heldWithdrawals.filter((h) => h.signer !== R2)
  return 'applied'
}

/** Bind W to the approval's root. Early withdrawals for W can never apply after this (rule 3). */
function bindKey(s: GuardianState, approval: Statement, now: number): WorkingKeyRecord {
  const rec: WorkingKeyRecord = {
    key: approval.key,
    root: approval.signer,
    status: 'active',
    label: approval.label,
    approvalId: approval.id,
    approvalIssuedAt: approval.issuedAt,
    departedBy: null,
    rebindRoot: null,
  }
  s.keys = s.keys.filter((k) => k.key !== approval.key)
  s.keys.push(rec)
  s.heldWithdrawals = s.heldWithdrawals.filter((h) => h.key !== approval.key)
  // An approval is sealed by the W it names (exception 1).
  putStatement(s, approval, approval.key, now)
  return rec
}

/**
 * An objection to removal `d` (of the root `d` names) against removals
 * signed by `remover`: it drops a removal whose effective time is at or below
 * its own, and any removal judged within 7 days of the objection reaching the
 * ward. Who signed it (the removed root or one of its working keys) was
 * checked when it was accepted.
 * OQ: Cancel or objection before its statement; OQ: Removal re-issued after an objection.
 */
function objectedBy(s: GuardianState, d: string, remover: string, effective: number, now: number): boolean {
  const o = storedFor(s, `${STATEMENT_PREFIXES.objection}${d}|${remover}`)
  return (
    !!o &&
    (effectiveTime(o.issuedAt, o.receivedAt) >= effective || now < o.receivedAt + OBJECTION_BLOCK_SECONDS)
  )
}

/**
 * A stored cancel for `d` from `signer` whose effective time is at or after
 * `effective`. OQ: Cancel or objection before its statement.
 */
function cancelledBy(s: GuardianState, d: string, signer: string, effective: number): boolean {
  const c = storedFor(s, `${STATEMENT_PREFIXES.cancel}${d}|${signer}`)
  return !!c && effectiveTime(c.issuedAt, c.receivedAt) >= effective
}

// ---------------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------------

/**
 * Advance timers to `now`, in time order (ties by `d`): a confirmed removal
 * takes effect at `takesEffectAt` if that is before its `expiresAt`;
 * otherwise a pending statement expires at `expiresAt`.
 * OQ: Removal that expires before it can take effect.
 */
function advance(s: GuardianState, now: number): TickEntry[] {
  const out: TickEntry[] = []
  s.tokens = s.tokens.filter((t) => now < t.mintedAt + TOKEN_LIFETIME_SECONDS)
  s.previousRelays = s.previousRelays.filter((p) => now < p.until)
  for (;;) {
    let best: { at: number; p: PendingRecord; act: 'apply' | 'expire' } | null = null
    for (const p of s.pending) {
      let due: { at: number; act: 'apply' | 'expire' } | null = null
      if (p.takesEffectAt !== null && p.takesEffectAt < p.expiresAt && p.takesEffectAt <= now) {
        due = { at: p.takesEffectAt, act: 'apply' }
      } else if (p.expiresAt <= now) {
        due = { at: p.expiresAt, act: 'expire' }
      }
      if (due && (!best || due.at < best.at || (due.at === best.at && cmp(p.d, best.p.d) < 0))) {
        best = { ...due, p }
      }
    }
    if (!best) break
    const p = best.p
    if (best.act === 'expire') {
      dropPending(s, (x) => x.d === p.d)
      out.push({ d: p.d, result: 'expired', reason: 'expired' })
    } else if (objectedBy(s, p.d, p.signer, effectiveTime(p.issuedAt, p.receivedAt), best.at)) {
      dropPending(s, (x) => x.d === p.d)
      out.push({ d: p.d, result: 'refused', reason: 'objected' })
    } else if (applyRemoval(s, p, effectiveTime(p.issuedAt, p.receivedAt), best.at) === 'applied') {
      out.push({ d: p.d, result: 'applied', reason: 'removal_applied' })
    } else {
      dropPending(s, (x) => x.d === p.d)
      out.push({ d: p.d, result: 'refused', reason: 'last_root' })
    }
  }
  return out
}

/** Run `body` against a timer-advanced copy and wrap the result. */
function step(
  state: GuardianState,
  now: number,
  body: (s: GuardianState) => { result: 'accepted' | 'dropped'; reason: ReasonCode },
): StepResult {
  const before = signerSet(state)
  const recipientsBefore = new Set(recipients(state))
  const s = clone(state)
  const ticked = advance(s, now)
  const { result, reason } = body(s)
  const after = new Set(signerSet(s))
  const departed = before.filter((k) => !after.has(k))
  for (const k of departed) if (recipientsBefore.has(k)) s.farewell.push(k)
  return { state: normalise(s), outcome: { result, reason, ticked, departed } }
}

const drop = (reason: ReasonCode) => ({ result: 'dropped' as const, reason })
const accept = (reason: ReasonCode) => ({ result: 'accepted' as const, reason })

/** Advance pending timers to `now` (expiry, the 72-hour removal notice, token lifetime). */
export function tick(state: GuardianState, now: number): StepResult {
  return step(state, now, () => accept('ticked'))
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/**
 * Who may seal a statement.
 * - An approval only by the W it names (exception 1).
 * - A withdrawal, co-guardian, removal, move or cancel signed by R by R, or by
 *   a W bound to R and in the signer set (exception 2).
 * - An objection by its signer, or by a W bound to the objecting root (the
 *   root being removed) and in the signer set (exception 2).
 */
function sealAllowed(s: GuardianState, st: Statement, sealAuthor: string): boolean {
  if (st.type === 'approval') return sealAuthor === st.key
  if (sealAuthor === st.signer) return true
  const w = activeKey(s, sealAuthor)
  return w !== undefined && w.root === (st.type === 'objection' ? st.key : st.signer)
}

function processApproval(s: GuardianState, st: Statement, seal: string, now: number) {
  // OQ: expiresAt on non-pending statements — an approval carrying one is refused.
  if (st.expiresAt !== null) return drop('expires_at_not_allowed')
  const W = st.key
  const R = st.signer
  if (isPinned(s, W) || W === R) return drop('key_role_conflict')
  // Rule 4: a removed or moved-away root never returns as a working key.
  if (isDepartedRoot(s, W)) return drop('key_departed')
  const rec = keyRecord(s, W)
  let mode: 'new' | 'update' | 'rebind' = 'new'
  if (rec) {
    if (rec.status === 'withdrawn') return drop('key_departed')
    if (rec.status === 'departed') {
      // OQ: Working keys after a root move or removal — only the root that
      // replaced the key's root by a confirmed move may re-bind it.
      if (rec.departedBy === 'move' && rec.rebindRoot === R) mode = 'rebind'
      else return drop('key_departed')
    } else if (rec.root !== R) {
      return drop('bound_elsewhere')
    } else {
      mode = 'update'
    }
  }
  const order = orderAgainst(s, st, seal, now)
  if (order) return drop(order)
  if (mode !== 'update') {
    if (rootAtQuota(s, R)) return drop('root_at_quota')
    if (activeCount(s, R) >= MAX_ACTIVE_KEYS_PER_ROOT) return drop('root_key_quota')
  }
  if (mode === 'update' && rec) {
    rec.label = st.label
    rec.approvalId = st.id
    rec.approvalIssuedAt = st.issuedAt
    putStatement(s, st, seal, now)
    return accept('approval_updated')
  }
  const held = s.heldWithdrawals.find((h) => h.key === W && h.signer === R)
  const bound = bindKey(s, st, now)
  if (held) {
    withdraw(s, bound, held, now)
    return accept('approval_preempted')
  }
  return accept(mode === 'rebind' ? 'approval_rebound' : 'approval_applied')
}

function processWithdrawal(s: GuardianState, st: Statement, now: number) {
  const W = st.key
  const S = st.signer
  if (isPinned(s, W)) return drop('key_role_conflict')
  const rec = keyRecord(s, W)
  if (rec) {
    if (rec.status === 'withdrawn') return drop('already_withdrawn')
    if (rec.status === 'active') {
      if (rec.root !== S) return drop('not_binding_root')
      withdraw(s, rec, st, now)
      return accept('withdrawal_applied')
    }
    // Departed: its old root (re-pinned since) or the root that may re-bind it
    // can still make the departure permanent. OQ: Working keys after a root
    // move or removal.
    if (rec.root === S || (rec.departedBy === 'move' && rec.rebindRoot === S)) {
      withdraw(s, rec, st, now)
      return accept('withdrawal_applied')
    }
    return drop('not_binding_root')
  }
  // Rule 3: not yet bound — keep it, since a pinned root signed it.
  const existing = s.heldWithdrawals.find((h) => h.key === W && h.signer === S)
  if (existing) return drop('duplicate')
  if (heldCount(s, S) >= MAX_HELD_PER_ROOT) return drop('held_quota')
  s.heldWithdrawals.push({ d: st.d, id: st.id, signer: S, key: W, issuedAt: st.issuedAt })
  return accept('withdrawal_held')
}

function addPending(s: GuardianState, st: Statement, type: PendingType, expiresAt: number, seal: string, now: number): void {
  dropPending(s, (p) => p.d === st.d)
  s.pending.push({
    d: st.d,
    type,
    id: st.id,
    signer: st.signer,
    key: st.key,
    issuedAt: st.issuedAt,
    receivedAt: now,
    expiresAt,
    confirmedAt: null,
    takesEffectAt: null,
  })
  putStatement(s, st, seal, now)
}

/**
 * Rule 7: re-pinning a departed root needs a statement whose effective time
 * is strictly after the departure's (both clamped to the ward clock, so a
 * far-future removal can't lock a parent out).
 */
function departedAtOrAfter(s: GuardianState, key: string, effective: number): boolean {
  const dep = s.departedRoots.find((d) => d.root === key)
  // A same-second removal wins. OQ: Same-second ties across different d.
  return !!dep && effective <= dep.issuedAt
}

function processCoGuardianOrMove(s: GuardianState, st: Statement, seal: string, now: number) {
  const R2 = st.key
  const effective = effectiveTime(st.issuedAt, now)
  if (isPinned(s, R2)) return drop('already_pinned')
  if (keyRecord(s, R2)) return drop('key_role_conflict')
  if (departedAtOrAfter(s, R2, effective)) return drop('not_newer_than_departure')
  const order = orderAgainst(s, st, seal, now)
  if (order) return drop(order)
  const expiresAt = pendingExpiry(effective, st.expiresAt)
  if (now >= expiresAt) return drop('expired')
  if (cancelledBy(s, st.d, st.signer, effective)) return drop('cancelled')
  if (st.type === 'coguardian' && s.roots.length >= MAX_ROOTS) return drop('root_quota')
  addPending(s, st, st.type as PendingType, expiresAt, seal, now)
  return accept(st.type === 'coguardian' ? 'coguardian_pending' : 'move_pending')
}

function processRemoval(s: GuardianState, st: Statement, seal: string, now: number) {
  const R2 = st.key
  const effective = effectiveTime(st.issuedAt, now)
  if (!isPinned(s, R2)) return drop('target_not_pinned')
  const order = orderAgainst(s, st, seal, now)
  if (order) return drop(order)
  if (st.signer === R2) {
    if (s.roots.filter((x) => x.root !== R2).length < 1) return drop('last_root')
    putStatement(s, st, seal, now)
    applyRemoval(s, st, effective, now)
    return accept('removal_applied')
  }
  const expiresAt = pendingExpiry(effective, st.expiresAt)
  if (now >= expiresAt) return drop('expired')
  if (objectedBy(s, st.d, st.signer, effective, now)) return drop('objected')
  if (cancelledBy(s, st.d, st.signer, effective)) return drop('cancelled')
  addPending(s, st, 'removal', expiresAt, seal, now)
  return accept('removal_pending')
}

function holdOrDrop(s: GuardianState, st: Statement, seal: string, now: number, reason: 'cancel_held' | 'objection_held') {
  const owner = heldOwner(st)
  const prev = storedFor(s, statementSlot(st))
  const reclaim = prev && prev.held && heldOwner(prev) === owner ? 1 : 0
  if (heldCount(s, owner) - reclaim >= MAX_HELD_PER_ROOT) return drop('held_quota')
  putStatement(s, st, seal, now, true)
  return accept(reason)
}

function processCancel(s: GuardianState, st: Statement, seal: string, now: number) {
  const order = orderAgainst(s, st, seal, now)
  if (order) return drop(order)
  const target = st.target as string
  const pend = s.pending.find((p) => p.d === target)
  if (pend && pend.signer !== st.signer) return drop('cancel_signer_mismatch')
  // At or below, by effective time: a same-second cancel wins. OQ: Same-second
  // ties across different d.
  if (pend && effectiveTime(pend.issuedAt, pend.receivedAt) <= effectiveTime(st.issuedAt, now)) {
    dropPending(s, (p) => p.d === target)
    putStatement(s, st, seal, now)
    return accept('cancel_applied')
  }
  return holdOrDrop(s, st, seal, now, 'cancel_held')
}

function processObjection(s: GuardianState, st: Statement, seal: string, now: number) {
  // Rule 7: the removed root, or an active W bound to it, may object, and
  // only against a removal by another pinned root: a removal's signer is
  // always pinned, so an objection naming any other `p` could only be held,
  // filling its root's quota.
  if (rootOf(s, st.signer) !== st.key || !isPinned(s, st.p) || st.p === st.key) return drop('objection_mismatch')
  const order = orderAgainst(s, st, seal, now)
  if (order) return drop(order)
  const target = st.target as string
  const pend = s.pending.find((p) => p.d === target)
  // Rule 7: an objection drops the pending removal by its `p`, whatever the
  // two issuedAt values.
  if (pend && pend.signer === st.p) {
    dropPending(s, (p) => p.d === target)
    putStatement(s, st, seal, now)
    return accept('objection_applied')
  }
  return holdOrDrop(s, st, seal, now, 'objection_held')
}

function processStatement(s: GuardianState, st: Statement, seal: string, now: number) {
  switch (st.type) {
    case 'approval':
      return processApproval(s, st, seal, now)
    case 'withdrawal':
      return processWithdrawal(s, st, now)
    case 'coguardian':
    case 'move':
      return processCoGuardianOrMove(s, st, seal, now)
    case 'removal':
      return processRemoval(s, st, seal, now)
    case 'cancel':
      return processCancel(s, st, seal, now)
    case 'objection':
      return processObjection(s, st, seal, now)
  }
}

/**
 * Apply one gift-wrapped statement (kind 31120) at the ward.
 *
 * `sealAuthor` is the authenticated seal author from the NIP-59 unwrap;
 * `inner` is the signed statement. Checks run in this order: statement
 * validity (`parseStatement`), the seal rule, the signer being a pinned
 * root, then the type's own checks. A statement dated ahead of the ward clock
 * is not refused: every comparison uses its effective time
 * (`effectiveTime`), and an accepted one counts in `clamped`.
 *
 * Presence confirmations (kind 31121) go to `applyPresenceConfirm`, not here.
 */
export function applyInbound(state: GuardianState, inbound: Inbound, now: number): StepResult {
  return step(state, now, (s) => {
    const parsed = parseStatement(inbound.inner)
    if (!parsed.ok) return drop(parsed.reason)
    const st = parsed.statement
    if (!sealAllowed(s, st, inbound.sealAuthor)) return drop('seal_mismatch')
    // Every statement is a root's, except an objection, which an active W
    // may sign for its root (rule 7).
    if (st.type === 'objection' ? !isSigner(s, st.signer) : !isPinned(s, st.signer)) return drop('signer_not_pinned')
    const r = processStatement(s, st, inbound.sealAuthor, now)
    if (r.result === 'accepted' && st.issuedAt > now) s.clamped += 1
    return r
  })
}

// ---------------------------------------------------------------------------
// Presence confirmation
// ---------------------------------------------------------------------------

/**
 * Record a presence token the ward minted on a privileged request (polkit on
 * Linux, the guardian PIN on Android). `statementD` binds the token to one
 * pending statement; null lets it confirm any. OQ: Presence confirmation details.
 */
export function registerPresenceToken(
  state: GuardianState,
  token: string,
  statementD: string | null,
  now: number,
): StepResult {
  return step(state, now, (s) => {
    if (!HEX32.test(token) || s.tokens.some((t) => t.token === token)) return drop('token_invalid')
    if (statementD !== null && !s.pending.some((p) => p.d === statementD)) return drop('not_pending')
    s.tokens.push({ token, statementD, mintedAt: now })
    return accept('token_registered')
  })
}

interface ConfirmContent {
  token: string
  statement: string
  approval: unknown
}

function parseConfirm(inner: unknown): ConfirmContent | null {
  if (!isEventShape(inner) || !verifySignedEvent(inner)) return null
  if (inner.kind !== CHARTER_PRESENCE_CONFIRM || !hasMarker(inner)) return null
  const parsed = tryParseStrictJson(inner.content)
  if (!parsed.ok) return null
  const c = parsed.value
  if (typeof c !== 'object' || c === null || Array.isArray(c)) return null
  const o = c as Record<string, unknown>
  if (o.v !== 1 || typeof o.token !== 'string' || !HEX32.test(o.token)) return null
  if (!isHex64(o.statement)) return null
  return { token: o.token, statement: o.statement, approval: o.approval }
}

/**
 * Apply a presence confirmation (kind 31121), signed and sealed by the
 * confirming working key.
 */
export function applyPresenceConfirm(state: GuardianState, inbound: Inbound, now: number): StepResult {
  return step(state, now, (s) => {
    const c = parseConfirm(inbound.inner)
    if (!c) return drop('bad_confirm')
    const W = inbound.inner.pubkey
    if (inbound.sealAuthor !== W) return drop('seal_mismatch')
    const pend = s.pending.find((p) => p.id === c.statement)
    if (!pend) return drop('not_pending')
    const tok = s.tokens.find(
      (t) => constantTimeEqual(t.token, c.token) && (t.statementD === null || t.statementD === pend.d),
    )
    if (!tok) return drop('token_invalid')
    const spend = () => {
      s.tokens = s.tokens.filter((t) => t.token !== tok.token)
    }

    if (pend.type === 'removal') {
      if (pend.confirmedAt !== null) return drop('duplicate')
      const rec = activeKey(s, W)
      if (!rec || rec.root !== pend.signer) return drop('confirmer_not_bound')
      const pendEffective = effectiveTime(pend.issuedAt, pend.receivedAt)
      if (objectedBy(s, pend.d, pend.signer, pendEffective, now)) {
        dropPending(s, (p) => p.d === pend.d)
        return drop('objected')
      }
      spend()
      pend.confirmedAt = now
      // Notice runs from the ward's own receipt (the effective time is never
      // later). OQ: Removal notice measured from receipt.
      pend.takesEffectAt = Math.max(now, pend.receivedAt + REMOVAL_NOTICE_SECONDS)
      if (pend.takesEffectAt <= now) {
        if (applyRemoval(s, pend, pendEffective, now) === 'last_root') {
          dropPending(s, (p) => p.d === pend.d)
          return drop('last_root')
        }
        return accept('removal_applied')
      }
      return accept('removal_confirmed')
    }

    // Co-guardian or move: the confirming W carries R2's approval of itself.
    // R2's role is checked again: it may have changed since the statement
    // arrived.
    const R2 = pend.key
    if (c.approval === undefined) return drop('approval_required')
    const pa = parseStatement(c.approval)
    if (!pa.ok) return drop('bad_approval')
    const ap = pa.statement
    if (ap.type !== 'approval' || ap.signer !== R2 || ap.key !== W) return drop('bad_approval')
    if (ap.expiresAt !== null) return drop('expires_at_not_allowed')
    if (isPinned(s, R2)) return drop('already_pinned')
    if (keyRecord(s, R2) || isPinned(s, W) || W === R2) return drop('key_role_conflict')
    const pendEffective = effectiveTime(pend.issuedAt, pend.receivedAt)
    if (departedAtOrAfter(s, R2, pendEffective)) return drop('not_newer_than_departure')
    if (isDepartedRoot(s, W)) return drop('key_departed')
    const rec = keyRecord(s, W)
    if (rec) {
      if (rec.status !== 'active') return drop('key_departed')
      if (pend.type === 'coguardian' || rec.root !== pend.signer) return drop('bound_elsewhere')
    }
    const order = orderAgainst(s, ap, W, now)
    if (order) return drop(order)
    if (rootAtQuota(s, R2)) return drop('root_at_quota')
    if (pend.type === 'coguardian' && s.roots.length >= MAX_ROOTS) return drop('root_quota')
    spend()
    if (ap.issuedAt > now) s.clamped += 1
    // Whichever statement pins R2 first, every other pending one naming R2 goes.
    dropPending(s, (p) => p.key === R2)

    if (pend.type === 'coguardian') {
      s.roots.push({ root: R2, pinnedAt: now, via: 'coguardian' })
      bindKey(s, ap, now)
      return accept('coguardian_applied')
    }

    // Rule 8: R2 replaces R. The confirming W re-binds to R2; R's other
    // working keys depart until R2 approves them. Pending and held statements
    // signed by R are dropped. OQ: Working keys after a root move or removal.
    const R = pend.signer
    const idx = s.roots.findIndex((r) => r.root === R)
    s.roots.splice(idx, 1, { root: R2, pinnedAt: now, via: 'move' })
    upsertDepartedRoot(s, R, pendEffective, 'move')
    const gone = [R]
    for (const k of s.keys) {
      if (k.status === 'active' && k.root === R && k.key !== W) {
        k.status = 'departed'
        k.departedBy = 'move'
        k.rebindRoot = R2
        gone.push(k.key)
      }
    }
    depart(s, gone, now)
    bindKey(s, ap, now)
    dropPending(s, (p) => p.signer === R || p.key === R)
    s.heldWithdrawals = s.heldWithdrawals.filter((h) => h.signer !== R)
    return accept('move_applied')
  })
}

// ---------------------------------------------------------------------------
// Clauses and other signed events
// ---------------------------------------------------------------------------

export interface ClauseInput {
  /** The CLAUSE event's id (64 lowercase hex). */
  id: string
  author: string
  sealAuthor: string
  subject: string
  kind: string
  issuedAt: number
}

/**
 * Rules 4 and 5 with the clock clamp: accept a CLAUSE if its author is in the
 * signer set, its `issuedAt` is above the bar of the held clause of the same
 * `(subject, kind)`, and above its author's own floor for that slot.
 *
 * - The bar is the held clause's effective time, min(`issuedAt`, the ward
 *   clock when it was accepted), never below the bar it replaced, and lowered
 *   to the departure time if its signer departs. A far-future clause therefore pins nothing: any later clause
 *   from a current signer replaces it.
 * - Each signer's floor is the highest `issuedAt` accepted from it for the
 *   slot. A signer's `issuedAt` only ever rises, so a clause at or below it
 *   is a re-delivery (`replay`): a relay can't replay a superseded far-future
 *   clause, whose real `issuedAt` beats the clamped bar. One floor per
 *   signer, so there is no quota to exhaust.
 *
 * Checks run: seal, signer, newer, replay.
 */
export function acceptClause(state: GuardianState, clause: ClauseInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (clause.sealAuthor !== clause.author) return drop('seal_mismatch')
    if (!isSigner(s, clause.author)) return drop('not_signer')
    const same = (c: HeldClause) => c.subject === clause.subject && c.kind === clause.kind
    const held = s.clauses.find(same)
    if (held && clause.issuedAt <= held.floor) return drop('not_newer')
    const mine = held ? clauseFloor(held, clause.author) : null
    if (mine !== null && clause.issuedAt <= mine) return drop('replay')
    // The new bar never falls below the old one, so a ward clock stepped back
    // can't reopen what the old bar closed.
    const effective = effectiveTime(clause.issuedAt, now)
    const bar = held ? Math.max(held.floor, effective) : effective
    const clamped = clause.issuedAt > now
    // The clause it replaces keeps its signer's floor (it may have been held
    // before floors were kept).
    let signerFloors = held?.signerFloors.map((f) => ({ ...f })) ?? []
    if (held && !goneForGood(s, held.signer)) signerFloors = raiseFloor(signerFloors, held.signer, held.issuedAt)
    signerFloors = raiseFloor(signerFloors, clause.author, clause.issuedAt)
    s.clauses = s.clauses.filter((c) => !same(c))
    s.clauses.push({
      subject: clause.subject,
      kind: clause.kind,
      signer: clause.author,
      root: rootOf(s, clause.author) as string,
      id: clause.id,
      issuedAt: clause.issuedAt,
      floor: bar,
      signerFloors,
    })
    if (clamped) s.clamped += 1
    return accept('clause_accepted')
  })
}

/** The clause a ward enforces for `(subject, kind)`, with who set it. */
export function heldClause(state: GuardianState, subject: string, kind: string): HeldClause | null {
  return state.clauses.find((c) => c.subject === subject && c.kind === kind) ?? null
}

export interface SignedEventInput {
  author: string
  sealAuthor: string
  /** The event's own timestamp (`ts`). */
  ts: number
}

/**
 * The signer-set gate for a GRANT or USAGE_SYNC: seal author is the inner
 * author and the author is in the signer set. `ts` is not judged here: a
 * GRANT keeps its own existing skew rule, and USAGE_SYNC has none.
 */
export function acceptSignedEvent(state: GuardianState, ev: SignedEventInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (ev.sealAuthor !== ev.author) return drop('seal_mismatch')
    if (!isSigner(s, ev.author)) return drop('not_signer')
    return accept('signed_accepted')
  })
}

export interface RelaysInput {
  author: string
  /**
   * `wrapped`: gift-wrapped over relays or the local link (then `sealAuthor`
   * is the seal's author). `pasted`: the offline relay code typed or pasted
   * at the ward (no seal).
   */
  channel: 'wrapped' | 'pasted'
  sealAuthor: string | null
  /** The event's `created_at`. */
  ts: number
  /**
   * The body's relay list, or undefined when the body has none (a RELAYS
   * carrying only the guardian PIN or battery settings).
   */
  relays?: string[]
  /** The optional `guardianPin` field of the RELAYS body. */
  guardianPin?: unknown
}

/** The same relay set, whatever the order. */
function sameRelays(a: readonly string[], b: readonly string[]): boolean {
  const x = [...a].sort(cmp)
  const y = [...b].sort(cmp)
  return x.length === y.length && x.every((r, i) => r === y[i])
}

/**
 * A RELAYS from the signer set (rule 11).
 *
 * - The existing RELAYS freshness checks: not in the future, at most 600 s
 *   old, not before pairing, newer than the last applied.
 * - A **PIN-only RELAYS** (gift-wrapped, a `guardianPin` and no relay list)
 *   has its own replay floor per signer, on its real `created_at`, and
 *   leaves the relay list's floor alone, so a PIN resend never makes a later
 *   relay code look already used. It is not refused for its age (a ward
 *   asleep still takes the PIN) nor for being ahead of the ward clock (a
 *   guardian phone a few minutes fast still sets it); it keeps the pairing
 *   floor. Battery settings in it are ignored by the ward.
 * - Any PIN is ordered like a clause: it is taken only if its `created_at`
 *   is above the PIN's bar, the effective time `min(created_at, now)` of the
 *   PIN in force, which never falls. So an older PIN from another install,
 *   processed later, can't roll back a newer one. A RELAYS with a relay
 *   list and a PIN at or below the bar or its author's PIN floor is refused
 *   whole.
 * - Only a pinned root may change the relay list. A working key's RELAYS must
 *   leave it as it is (no list, or the same set): it may carry the PIN and
 *   battery settings. A stolen phone can't steer a ward away from the relays
 *   its withdrawal is published on.
 * - A `guardianPin` is accepted only over the wrapped channel.
 *
 * Relay-list validation (2–10 distinct secure endpoints) and the battery
 * settings are the ward's existing checks, not modelled here.
 */
export function applyRelays(state: GuardianState, ev: RelaysInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (ev.channel === 'wrapped' && ev.sealAuthor !== ev.author) return drop('seal_mismatch')
    if (!isSigner(s, ev.author)) return drop('not_signer')
    const pinOnly = ev.channel === 'wrapped' && ev.relays === undefined && ev.guardianPin !== undefined
    if (pinOnly) {
      if (ev.ts < s.pairedAt) return drop('pairing_floor')
      const floor = s.pinFloors.find((f) => f.signer === ev.author)
      if (floor && ev.ts <= floor.issuedAt) return drop('not_newer')
      if (ev.ts <= s.pinBar) return drop('not_newer')
      const v = validateGuardianPinRecord(ev.guardianPin)
      if (!v.ok) return drop('bad_pin_record')
      setPin(s, v.record, ev.author, ev.ts, now)
      return accept('relays_applied')
    }
    if (ev.ts > now) return drop('future')
    if (now - ev.ts > RELAYS_TTL_SECONDS) return drop('stale')
    if (ev.ts < s.pairedAt) return drop('pairing_floor')
    if (ev.ts <= s.relaysUpdatedAt) return drop('not_newer')
    const changes = ev.relays !== undefined && !sameRelays(ev.relays, s.relays)
    if (changes && !isPinned(s, ev.author)) return drop('relays_root_only')
    let pin: GuardianPinRecord | null = null
    if (ev.guardianPin !== undefined) {
      if (ev.channel !== 'wrapped') return drop('pin_not_wrapped')
      const v = validateGuardianPinRecord(ev.guardianPin)
      if (!v.ok) return drop('bad_pin_record')
      const floor = s.pinFloors.find((f) => f.signer === ev.author)
      if ((floor && ev.ts <= floor.issuedAt) || ev.ts <= s.pinBar) return drop('not_newer')
      pin = v.record
    }
    if (changes && ev.relays) {
      s.previousRelays.push({ relays: s.relays, until: now + PREVIOUS_RELAYS_SECONDS })
      while (s.previousRelays.length > PREVIOUS_RELAYS_MAX) s.previousRelays.shift()
      s.relays = [...ev.relays]
    }
    s.relaysUpdatedAt = ev.ts
    if (pin) setPin(s, pin, ev.author, ev.ts, now)
    return accept('relays_applied')
  })
}

/** Apply an accepted PIN: the record, its author's floor, the bar (never falling) and `pinSetAt`. */
function setPin(s: GuardianState, record: GuardianPinRecord, author: string, ts: number, now: number): void {
  s.guardianPin = record
  s.pinFloors = raiseFloor(s.pinFloors, author, ts)
  s.pinBar = Math.max(s.pinBar, Math.min(ts, now))
  s.pinSetAt = ts
}

/** The ward has sent STATUS: departed keys have had their last one. */
export function markStatusSent(state: GuardianState, now: number): StepResult {
  return step(state, now, (s) => {
    s.farewell = []
    return accept('status_sent')
  })
}

// ---------------------------------------------------------------------------
// Scan-to-pair with a working key
// ---------------------------------------------------------------------------

export interface PairToken {
  token: string
  mintedAt: number
}

export type PairResult =
  | { ok: true; reason: 'paired'; state: GuardianState; root: string; workingKey: string | null }
  | { ok: false; reason: ReasonCode }

/**
 * Pair an unpaired ward from a PAIR_OFFER (kind 31117).
 *
 * - An offer carrying `approval` must be sealed by the W the approval names,
 *   and signed (the approval) by R: the ward pins R and binds W.
 * - An offer with no approval pins its seal author as the root, as today (a
 *   ward cannot tell a W from a root by its key).
 * - Nothing is ever pinned from a key read out of the payload.
 *
 * Token rules are those of scan-to-pair: constant-time compare, 600-second
 * lifetime against the mint time and the offer's `ts`, a future `ts` refused.
 * A contested token (two keys, one token) is the caller's to detect.
 */
export function pairFromOffer(inbound: Inbound, token: PairToken, now: number): PairResult {
  const inner = inbound.inner
  if (!isEventShape(inner) || !verifySignedEvent(inner)) return { ok: false, reason: 'bad_offer' }
  if (inner.kind !== CHARTER_DEVICE_PAIR_OFFER || !hasMarker(inner)) return { ok: false, reason: 'bad_offer' }
  if (inbound.sealAuthor !== inner.pubkey) return { ok: false, reason: 'seal_mismatch' }
  const parsed = tryParseStrictJson(inner.content)
  if (!parsed.ok) return { ok: false, reason: 'bad_offer' }
  const c = parsed.value
  if (typeof c !== 'object' || c === null || Array.isArray(c)) return { ok: false, reason: 'bad_offer' }
  const o = c as Record<string, unknown>
  if (typeof o.token !== 'string' || !HEX32.test(o.token)) return { ok: false, reason: 'bad_offer' }
  if (!Array.isArray(o.relays) || !o.relays.every((r) => typeof r === 'string')) {
    return { ok: false, reason: 'bad_offer' }
  }
  if (!isUnixSeconds(o.ts)) return { ok: false, reason: 'bad_offer' }
  if (
    !constantTimeEqual(token.token, o.token) ||
    now >= token.mintedAt + TOKEN_LIFETIME_SECONDS ||
    o.ts > now ||
    now - o.ts >= TOKEN_LIFETIME_SECONDS
  ) {
    return { ok: false, reason: 'token_invalid' }
  }

  const state: GuardianState = {
    v: 1,
    pairedAt: now,
    roots: [],
    keys: [],
    statements: [],
    statementFloors: [],
    pending: [],
    heldWithdrawals: [],
    tombstones: [],
    departedRoots: [],
    tokens: [],
    clauses: [],
    relays: [...(o.relays as string[])],
    relaysUpdatedAt: 0,
    previousRelays: [],
    guardianPin: null,
    pinFloors: [],
    pinBar: 0,
    pinSetAt: null,
    farewell: [],
    clamped: 0,
  }

  if (o.approval === undefined) {
    state.roots.push({ root: inbound.sealAuthor, pinnedAt: now, via: 'pair' })
    return { ok: true, reason: 'paired', state, root: inbound.sealAuthor, workingKey: null }
  }
  const pa = parseStatement(o.approval)
  if (!pa.ok || pa.statement.type !== 'approval') return { ok: false, reason: 'bad_approval' }
  const ap = pa.statement
  if (ap.key !== inbound.sealAuthor) return { ok: false, reason: 'seal_mismatch' }
  if (ap.signer === ap.key) return { ok: false, reason: 'key_role_conflict' }
  if (ap.expiresAt !== null) return { ok: false, reason: 'expires_at_not_allowed' }
  if (ap.issuedAt > now) state.clamped = 1
  state.roots.push({ root: ap.signer, pinnedAt: now, via: 'pair' })
  bindKey(state, ap, now)
  return { ok: true, reason: 'paired', state: normalise(state), root: ap.signer, workingKey: ap.key }
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

export interface StatusGuardians {
  roots: { root: string; keys: string[] }[]
  /**
   * `id` is the pending statement's event id (a typed presence code names
   * it); `signer` is the key that signed it (an objection's `p` is a
   * removal's signer).
   */
  pending: { id: string; d: string; kind: PendingType; signer: string; takesEffectAt?: number; expiresAt: number }[]
  tombstones: number
  relays: string[]
  /** A guardian PIN is set (Android presence confirmation needs one). */
  pinSet: boolean
  /**
   * The `created_at` of the RELAYS that set the PIN in force; absent when
   * none is set. A guardian install marks its PIN set only when this is the
   * `created_at` it sent.
   */
  pinSetAt?: number
  /** Accepted clauses and statements whose claimed time was clamped to the ward clock, since pairing. */
  clamped: number
}

/** The `guardians` object a ward adds to STATUS. */
export function statusGuardians(state: GuardianState): StatusGuardians {
  return {
    roots: state.roots.map((r) => ({
      root: r.root,
      keys: state.keys
        .filter((k) => k.status === 'active' && k.root === r.root)
        .map((k) => k.key)
        .sort(cmp),
    })),
    pending: state.pending.map((p) => ({
      id: p.id,
      d: p.d,
      kind: p.type,
      signer: p.signer,
      ...(p.takesEffectAt !== null ? { takesEffectAt: p.takesEffectAt } : {}),
      expiresAt: p.expiresAt,
    })),
    tombstones: state.tombstones.length,
    relays: [...state.relays],
    pinSet: state.guardianPin !== null,
    ...(state.guardianPin !== null && state.pinSetAt !== null ? { pinSetAt: state.pinSetAt } : {}),
    clamped: state.clamped,
  }
}

/** The projected state recorded after every step of the golden scenarios. */
export interface ProjectedState {
  roots: { root: string; keys: string[] }[]
  signers: { key: string; root: string }[]
  pending: {
    d: string
    kind: PendingType
    signer: string
    statement: string
    issuedAt: number
    receivedAt: number
    expiresAt: number
    confirmedAt: number | null
    takesEffectAt: number | null
  }[]
  tombstones: number
  held: number
  departed: string[]
  recipients: string[]
  farewell: string[]
  clauses: HeldClause[]
  relays: string[]
  statusRelays: string[]
  guardianPin: GuardianPinRecord | null
  pinSetAt: number | null
  pinBar: number
  clamped: number
}

export function projectState(state: GuardianState, now: number): ProjectedState {
  const departed = new Set<string>()
  for (const k of state.keys) if (isDeparted(state, k.key)) departed.add(k.key)
  for (const d of state.departedRoots) if (isDeparted(state, d.root)) departed.add(d.root)
  return {
    roots: statusGuardians(state).roots,
    signers: signerSet(state).map((key) => ({ key, root: rootOf(state, key) as string })),
    pending: state.pending.map((p) => ({
      d: p.d,
      kind: p.type,
      signer: p.signer,
      statement: p.id,
      issuedAt: p.issuedAt,
      receivedAt: p.receivedAt,
      expiresAt: p.expiresAt,
      confirmedAt: p.confirmedAt,
      takesEffectAt: p.takesEffectAt,
    })),
    tombstones: state.tombstones.length,
    held: state.heldWithdrawals.length + state.statements.filter((s) => s.held).length,
    departed: [...departed].sort(cmp),
    recipients: recipients(state),
    farewell: [...state.farewell],
    clauses: state.clauses.map((c) => ({ ...c, signerFloors: c.signerFloors.map((f) => ({ ...f })) })),
    relays: [...state.relays],
    statusRelays: statusRelays(state, now),
    guardianPin: state.guardianPin ? { ...state.guardianPin } : null,
    pinSetAt: state.pinSetAt,
    pinBar: state.pinBar,
    clamped: state.clamped,
  }
}

/** Convenience for callers holding a signed event that isn't wrapped. */
export type { SignedEvent }
