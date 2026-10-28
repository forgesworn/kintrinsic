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
  FUTURE_SKEW_SECONDS,
  HEX32,
  MAX_ACTIVE_KEYS_PER_ROOT,
  MAX_HELD_PER_ROOT,
  MAX_ROOTS,
  MAX_TOMBSTONES_PER_ROOT,
  OBJECTION_BLOCK_SECONDS,
  PENDING_MAX_SECONDS,
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

/** A held clause: the minimal model of rules 4 and 5. */
export interface HeldClause {
  subject: string
  kind: string
  signer: string
  /** The signer's root at acceptance, for "set by" display (rule 5). */
  root: string
  issuedAt: number
  /**
   * Set when the signer departs: min(`issuedAt`, the departure time). While
   * the signer is departed, a replacement must be newer than this.
   * OQ: Clause floor after a departure.
   */
  floor: number | null
}

export interface GuardianState {
  v: 1
  pairedAt: number
  roots: PinnedRoot[]
  keys: WorkingKeyRecord[]
  statements: StoredStatement[]
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
  /** Departed keys owed one last STATUS showing their departure. */
  farewell: string[]
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
  return JSON.parse(JSON.stringify(s)) as GuardianState
}

/** Sort every list whose order carries no meaning, so JSON is stable. */
function normalise(s: GuardianState): GuardianState {
  s.keys.sort((a, b) => cmp(a.key, b.key))
  s.statements.sort((a, b) => cmp(a.slot, b.slot))
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

function heldCount(s: GuardianState, signer: string): number {
  return (
    s.heldWithdrawals.filter((h) => h.signer === signer).length +
    s.statements.filter((st) => st.held && st.signer === signer).length
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

/** Pending expiry: `expiresAt` clamped to 7 days after `issuedAt`; 7 days without one. */
export function pendingExpiry(issuedAt: number, expiresAt: number | null): number {
  const cap = issuedAt + PENDING_MAX_SECONDS
  return expiresAt === null ? cap : Math.min(expiresAt, cap)
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

function putStatement(s: GuardianState, st: Statement, now: number, held = false): void {
  if (st.type === 'withdrawal') return
  const slot = statementSlot(st)
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
  })
}

/**
 * Rule 2 within one slot: the newest `issuedAt` wins. Re-delivery of the
 * held event is `duplicate`; an older one is `stale`. At the same `issuedAt`
 * the lower event id wins, so every ward converges whatever the arrival
 * order; the loser is `tie_not_newer`.
 * OQ: Same d, same issuedAt, different content.
 */
function orderAgainst(stored: { id: string; issuedAt: number } | undefined, st: Statement): ReasonCode | null {
  if (!stored) return null
  if (stored.id === st.id) return 'duplicate'
  if (st.issuedAt < stored.issuedAt) return 'stale'
  if (st.issuedAt === stored.issuedAt && cmp(st.id, stored.id) > 0) return 'tie_not_newer'
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
    if (keys.includes(c.signer)) c.floor = Math.min(c.floor ?? c.issuedAt, c.issuedAt, at)
  }
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
  clampClauses(s, [rec.key], at)
}

/**
 * Apply a root removal (rule 7) at time `at`. Refused unless another,
 * distinct root remains: removal never leaves a ward with no roots.
 */
function applyRemoval(
  s: GuardianState,
  r: { d: string; id: string; signer: string; key: string; issuedAt: number },
  at: number,
): 'applied' | 'last_root' {
  const R2 = r.key
  if (!isPinned(s, R2) || s.roots.filter((x) => x.root !== R2).length < 1) return 'last_root'
  s.roots = s.roots.filter((x) => x.root !== R2)
  upsertDepartedRoot(s, R2, r.issuedAt, 'removal')
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
  clampClauses(s, gone, at)
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
  putStatement(s, approval, now)
  return rec
}

/**
 * An objection by `removed` against removals signed by `remover`: it drops a
 * removal arriving later whose `issuedAt` is at or below its own, and any
 * removal arriving within 7 days of the objection reaching the ward.
 * OQ: Cancel or objection before its statement; OQ: Removal re-issued after an objection.
 */
function objectedBy(s: GuardianState, d: string, removed: string, remover: string, issuedAt: number, now: number): boolean {
  const o = storedFor(s, `${STATEMENT_PREFIXES.objection}${d}|${remover}`)
  return !!o && o.signer === removed && (o.issuedAt >= issuedAt || now < o.receivedAt + OBJECTION_BLOCK_SECONDS)
}

/** A held cancel for `d` from `signer` at or after `issuedAt`. OQ: Cancel or objection before its statement. */
function cancelledBy(s: GuardianState, d: string, signer: string, issuedAt: number): boolean {
  const c = storedFor(s, `${STATEMENT_PREFIXES.cancel}${d}|${signer}`)
  return !!c && c.issuedAt >= issuedAt
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
    } else if (objectedBy(s, p.d, p.key, p.signer, p.issuedAt, best.at)) {
      dropPending(s, (x) => x.d === p.d)
      out.push({ d: p.d, result: 'refused', reason: 'objected' })
    } else if (applyRemoval(s, p, best.at) === 'applied') {
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
 * - Anything else (an objection) only by its signer. OQ: Sealing an objection.
 */
function sealAllowed(s: GuardianState, st: Statement, sealAuthor: string): boolean {
  if (st.type === 'approval') return sealAuthor === st.key
  if (sealAuthor === st.signer) return true
  if (st.type === 'objection') return false
  const w = activeKey(s, sealAuthor)
  return w !== undefined && w.root === st.signer
}

function processApproval(s: GuardianState, st: Statement, now: number) {
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
  const order = orderAgainst(storedFor(s, st.d), st)
  if (order) return drop(order)
  if (mode !== 'update') {
    if (rootAtQuota(s, R)) return drop('root_at_quota')
    if (activeCount(s, R) >= MAX_ACTIVE_KEYS_PER_ROOT) return drop('root_key_quota')
  }
  if (mode === 'update' && rec) {
    rec.label = st.label
    rec.approvalId = st.id
    rec.approvalIssuedAt = st.issuedAt
    putStatement(s, st, now)
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

function addPending(s: GuardianState, st: Statement, type: PendingType, expiresAt: number, now: number): void {
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
  putStatement(s, st, now)
}

/** Rule 7: re-pinning a departed root needs a statement strictly newer than its departure. */
function departedAtOrAfter(s: GuardianState, key: string, issuedAt: number): boolean {
  const dep = s.departedRoots.find((d) => d.root === key)
  // A same-second removal wins. OQ: Same-second ties across different d.
  return !!dep && issuedAt <= dep.issuedAt
}

function processCoGuardianOrMove(s: GuardianState, st: Statement, now: number) {
  const R2 = st.key
  if (isPinned(s, R2)) return drop('already_pinned')
  if (keyRecord(s, R2)) return drop('key_role_conflict')
  if (departedAtOrAfter(s, R2, st.issuedAt)) return drop('not_newer_than_departure')
  const order = orderAgainst(storedFor(s, st.d), st)
  if (order) return drop(order)
  const expiresAt = pendingExpiry(st.issuedAt, st.expiresAt)
  if (now >= expiresAt) return drop('expired')
  if (cancelledBy(s, st.d, st.signer, st.issuedAt)) return drop('cancelled')
  if (st.type === 'coguardian' && s.roots.length >= MAX_ROOTS) return drop('root_quota')
  addPending(s, st, st.type as PendingType, expiresAt, now)
  return accept(st.type === 'coguardian' ? 'coguardian_pending' : 'move_pending')
}

function processRemoval(s: GuardianState, st: Statement, now: number) {
  const R2 = st.key
  if (!isPinned(s, R2)) return drop('target_not_pinned')
  const order = orderAgainst(storedFor(s, st.d), st)
  if (order) return drop(order)
  if (st.signer === R2) {
    if (s.roots.filter((x) => x.root !== R2).length < 1) return drop('last_root')
    putStatement(s, st, now)
    applyRemoval(s, st, now)
    return accept('removal_applied')
  }
  const expiresAt = pendingExpiry(st.issuedAt, st.expiresAt)
  if (now >= expiresAt) return drop('expired')
  if (objectedBy(s, st.d, R2, st.signer, st.issuedAt, now)) return drop('objected')
  if (cancelledBy(s, st.d, st.signer, st.issuedAt)) return drop('cancelled')
  addPending(s, st, 'removal', expiresAt, now)
  return accept('removal_pending')
}

function holdOrDrop(s: GuardianState, st: Statement, now: number, reason: 'cancel_held' | 'objection_held') {
  const prev = storedFor(s, statementSlot(st))
  const reclaim = prev && prev.held && prev.signer === st.signer ? 1 : 0
  if (heldCount(s, st.signer) - reclaim >= MAX_HELD_PER_ROOT) return drop('held_quota')
  putStatement(s, st, now, true)
  return accept(reason)
}

function processCancel(s: GuardianState, st: Statement, now: number) {
  const order = orderAgainst(storedFor(s, statementSlot(st)), st)
  if (order) return drop(order)
  const target = st.target as string
  const pend = s.pending.find((p) => p.d === target)
  if (pend && pend.signer !== st.signer) return drop('cancel_signer_mismatch')
  // At or below: a same-second cancel wins. OQ: Same-second ties across different d.
  if (pend && pend.issuedAt <= st.issuedAt) {
    dropPending(s, (p) => p.d === target)
    putStatement(s, st, now)
    return accept('cancel_applied')
  }
  return holdOrDrop(s, st, now, 'cancel_held')
}

function processObjection(s: GuardianState, st: Statement, now: number) {
  if (st.signer !== st.key) return drop('objection_mismatch')
  const order = orderAgainst(storedFor(s, statementSlot(st)), st)
  if (order) return drop(order)
  const target = st.target as string
  const pend = s.pending.find((p) => p.d === target)
  // Rule 7: an objection drops the pending removal by its `p`, whatever the
  // two issuedAt values.
  if (pend && pend.signer === st.p) {
    dropPending(s, (p) => p.d === target)
    putStatement(s, st, now)
    return accept('objection_applied')
  }
  return holdOrDrop(s, st, now, 'objection_held')
}

/**
 * Apply one gift-wrapped statement (kind 31120) at the ward.
 *
 * `sealAuthor` is the authenticated seal author from the NIP-59 unwrap;
 * `inner` is the signed statement. Checks run in this order: statement
 * validity (`parseStatement`), the 300-second future refusal, the seal rule,
 * the signer being a pinned root, then the type's own checks.
 *
 * Presence confirmations (kind 31121) go to `applyPresenceConfirm`, not here.
 */
export function applyInbound(state: GuardianState, inbound: Inbound, now: number): StepResult {
  return step(state, now, (s) => {
    const parsed = parseStatement(inbound.inner)
    if (!parsed.ok) return drop(parsed.reason)
    const st = parsed.statement
    if (st.issuedAt > now + FUTURE_SKEW_SECONDS) return drop('future')
    if (!sealAllowed(s, st, inbound.sealAuthor)) return drop('seal_mismatch')
    if (!isPinned(s, st.signer)) return drop('signer_not_pinned')
    switch (st.type) {
      case 'approval':
        return processApproval(s, st, now)
      case 'withdrawal':
        return processWithdrawal(s, st, now)
      case 'coguardian':
      case 'move':
        return processCoGuardianOrMove(s, st, now)
      case 'removal':
        return processRemoval(s, st, now)
      case 'cancel':
        return processCancel(s, st, now)
      case 'objection':
        return processObjection(s, st, now)
    }
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
      if (objectedBy(s, pend.d, pend.key, pend.signer, pend.issuedAt, now)) {
        dropPending(s, (p) => p.d === pend.d)
        return drop('objected')
      }
      spend()
      pend.confirmedAt = now
      // Notice runs from the later of issuedAt and the ward's own receipt.
      // OQ: Removal notice measured from receipt.
      pend.takesEffectAt = Math.max(now, Math.max(pend.issuedAt, pend.receivedAt) + REMOVAL_NOTICE_SECONDS)
      if (pend.takesEffectAt <= now) {
        if (applyRemoval(s, pend, now) === 'last_root') {
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
    if (ap.issuedAt > now + FUTURE_SKEW_SECONDS) return drop('future')
    if (isPinned(s, R2)) return drop('already_pinned')
    if (keyRecord(s, R2) || isPinned(s, W) || W === R2) return drop('key_role_conflict')
    if (departedAtOrAfter(s, R2, pend.issuedAt)) return drop('not_newer_than_departure')
    if (isDepartedRoot(s, W)) return drop('key_departed')
    const rec = keyRecord(s, W)
    if (rec) {
      if (rec.status !== 'active') return drop('key_departed')
      if (pend.type === 'coguardian' || rec.root !== pend.signer) return drop('bound_elsewhere')
    }
    const order = orderAgainst(storedFor(s, ap.d), ap)
    if (order) return drop(order)
    if (rootAtQuota(s, R2)) return drop('root_at_quota')
    if (pend.type === 'coguardian' && s.roots.length >= MAX_ROOTS) return drop('root_quota')
    spend()
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
    upsertDepartedRoot(s, R, pend.issuedAt, 'move')
    const gone = [R]
    for (const k of s.keys) {
      if (k.status === 'active' && k.root === R && k.key !== W) {
        k.status = 'departed'
        k.departedBy = 'move'
        k.rebindRoot = R2
        gone.push(k.key)
      }
    }
    clampClauses(s, gone, now)
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
  author: string
  sealAuthor: string
  subject: string
  kind: string
  issuedAt: number
}

/**
 * Rules 4 and 5: accept a CLAUSE if its author is in the signer set and it is
 * newer than the held clause of the same `(subject, kind)`: newer than its
 * `issuedAt` while its signer is current, newer than its departure floor once
 * the signer has departed. Checks run: future, seal, signer, pairing floor,
 * newer.
 */
export function acceptClause(state: GuardianState, clause: ClauseInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (clause.issuedAt > now + FUTURE_SKEW_SECONDS) return drop('future')
    if (clause.sealAuthor !== clause.author) return drop('seal_mismatch')
    if (!isSigner(s, clause.author)) return drop('not_signer')
    if (clause.issuedAt < s.pairedAt) return drop('pairing_floor')
    const held = s.clauses.find((c) => c.subject === clause.subject && c.kind === clause.kind)
    if (held) {
      const bar = isSigner(s, held.signer) ? held.issuedAt : (held.floor ?? held.issuedAt)
      if (clause.issuedAt <= bar) return drop('not_newer')
    }
    s.clauses = s.clauses.filter((c) => !(c.subject === clause.subject && c.kind === clause.kind))
    s.clauses.push({
      subject: clause.subject,
      kind: clause.kind,
      signer: clause.author,
      root: rootOf(s, clause.author) as string,
      issuedAt: clause.issuedAt,
      floor: null,
    })
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
 * author, the author is in the signer set, and `ts` is not more than 300
 * seconds ahead (OQ: 300-second future refusal for GRANT, RELAYS and
 * USAGE_SYNC). Their own existing checks still apply on top.
 */
export function acceptSignedEvent(state: GuardianState, ev: SignedEventInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (ev.ts > now + FUTURE_SKEW_SECONDS) return drop('future')
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
  relays: string[]
  /** The optional `guardianPin` field of the RELAYS body. */
  guardianPin?: unknown
}

/**
 * A RELAYS change from any signer (rule 11), with the existing RELAYS
 * freshness checks: not in the future, at most 600 s old, not before pairing,
 * newer than the last applied. A `guardianPin` is accepted only over the
 * wrapped channel. OQ: How a guardian PIN reaches an Android ward.
 * Relay-list validation (2–10 distinct secure endpoints) is not modelled.
 */
export function applyRelays(state: GuardianState, ev: RelaysInput, now: number): StepResult {
  return step(state, now, (s) => {
    if (ev.channel === 'wrapped' && ev.sealAuthor !== ev.author) return drop('seal_mismatch')
    if (!isSigner(s, ev.author)) return drop('not_signer')
    if (ev.ts > now) return drop('future')
    if (now - ev.ts > RELAYS_TTL_SECONDS) return drop('stale')
    if (ev.ts < s.pairedAt) return drop('pairing_floor')
    if (ev.ts <= s.relaysUpdatedAt) return drop('not_newer')
    let pin: GuardianPinRecord | null = null
    if (ev.guardianPin !== undefined) {
      if (ev.channel !== 'wrapped') return drop('pin_not_wrapped')
      const v = validateGuardianPinRecord(ev.guardianPin)
      if (!v.ok) return drop('bad_pin_record')
      pin = v.record
    }
    s.previousRelays.push({ relays: s.relays, until: now + PREVIOUS_RELAYS_SECONDS })
    s.relays = [...ev.relays]
    s.relaysUpdatedAt = ev.ts
    if (pin) s.guardianPin = pin
    return accept('relays_applied')
  })
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
    farewell: [],
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
  if (ap.issuedAt > now + FUTURE_SKEW_SECONDS) return { ok: false, reason: 'future' }
  state.roots.push({ root: ap.signer, pinnedAt: now, via: 'pair' })
  bindKey(state, ap, now)
  return { ok: true, reason: 'paired', state: normalise(state), root: ap.signer, workingKey: ap.key }
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

export interface StatusGuardians {
  roots: { root: string; keys: string[] }[]
  pending: { d: string; kind: PendingType; takesEffectAt?: number; expiresAt: number }[]
  tombstones: number
  relays: string[]
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
      d: p.d,
      kind: p.type,
      ...(p.takesEffectAt !== null ? { takesEffectAt: p.takesEffectAt } : {}),
      expiresAt: p.expiresAt,
    })),
    tombstones: state.tombstones.length,
    relays: [...state.relays],
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
    clauses: state.clauses.map((c) => ({ ...c })),
    relays: [...state.relays],
    statusRelays: statusRelays(state, now),
    guardianPin: state.guardianPin ? { ...state.guardianPin } : null,
  }
}

/** Convenience for callers holding a signed event that isn't wrapped. */
export type { SignedEvent }
