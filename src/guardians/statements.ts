/**
 * Guardian statements (kind 31120): parsing and validation.
 *
 * `parseStatement` checks everything a statement must satisfy on its own,
 * before any ward state is consulted: the event's `id` and `sig`, the kind,
 * the marker tag, exactly one `d` and one `p`, their agreement, and the
 * content `{ v: 1, issuedAt, label?, expiresAt? }`. Unknown content fields are
 * ignored and `created_at` plays no part. Who may sign or seal which statement
 * depends on the ward's state and is checked in `state.ts`.
 */
import { verifyEvent, type NostrEvent } from 'nostr-tools/pure'
import type { StatementReason } from './reasons.js'
import { tryParseStrictJson } from './json.js'
import {
  CHARTER_GUARDIAN_STATEMENT,
  MAX_LABEL_BYTES,
  isHex64,
  isUnixSeconds,
  utf8Length,
  type SignedEvent,
} from './types.js'

export type StatementType =
  | 'approval'
  | 'withdrawal'
  | 'coguardian'
  | 'removal'
  | 'move'
  | 'cancel'
  | 'objection'

/** The `d` prefix of each statement type. */
export const STATEMENT_PREFIXES: Readonly<Record<StatementType, string>> = {
  approval: 'kintrinsic/device/',
  withdrawal: 'kintrinsic/revoke/',
  coguardian: 'kintrinsic/guardian/',
  removal: 'kintrinsic/unguardian/',
  move: 'kintrinsic/root/',
  cancel: 'kintrinsic/cancel/',
  objection: 'kintrinsic/object/',
}

/** Statement types that can be pending (and so cancelled). */
export type PendingType = 'coguardian' | 'removal' | 'move'

const SIMPLE_TYPES: StatementType[] = ['approval', 'withdrawal', 'coguardian', 'removal', 'move']
const PENDING_TYPES: PendingType[] = ['coguardian', 'removal', 'move']

/** A statement that passed `parseStatement`. */
export interface Statement {
  type: StatementType
  /** The event's single `d` tag. */
  d: string
  /** The event's single `p` tag. */
  p: string
  /**
   * The key the statement is about: W for an approval or withdrawal, R2 for a
   * co-guardian, removal or move, the pending statement's R2 for a cancel, and
   * the root being removed for an objection.
   */
  key: string
  /** For a cancel or objection: the `d` of the statement it targets. */
  target: string | null
  /** For a cancel: the type of the statement it targets. */
  targetType: PendingType | null
  /** The inner event's author. */
  signer: string
  /** The inner event's id. */
  id: string
  issuedAt: number
  label: string | null
  expiresAt: number | null
  event: SignedEvent
}

export type ParseStatementResult =
  | { ok: true; statement: Statement }
  | { ok: false; reason: StatementReason }

/** Split a `d` into its type and the key or target it names. */
function splitD(d: string): { type: StatementType; rest: string } | null {
  for (const type of Object.keys(STATEMENT_PREFIXES) as StatementType[]) {
    const prefix = STATEMENT_PREFIXES[type]
    if (d.startsWith(prefix)) return { type, rest: d.slice(prefix.length) }
  }
  return null
}

/** Split a pending statement's `d` (co-guardian, removal or move). */
export function splitPendingD(d: string): { type: PendingType; key: string } | null {
  const split = splitD(d)
  if (!split || !PENDING_TYPES.includes(split.type as PendingType)) return null
  if (!isHex64(split.rest)) return null
  return { type: split.type as PendingType, key: split.rest }
}

/** Shape check for a NIP-01 event object, before signature verification. */
export function isEventShape(x: unknown): x is SignedEvent {
  if (typeof x !== 'object' || x === null || Array.isArray(x)) return false
  const e = x as Record<string, unknown>
  return (
    typeof e.id === 'string' &&
    /^[0-9a-f]{64}$/.test(e.id) &&
    typeof e.pubkey === 'string' &&
    /^[0-9a-f]{64}$/.test(e.pubkey) &&
    typeof e.sig === 'string' &&
    /^[0-9a-f]{128}$/.test(e.sig) &&
    typeof e.kind === 'number' &&
    Number.isSafeInteger(e.kind) &&
    typeof e.created_at === 'number' &&
    Number.isSafeInteger(e.created_at) &&
    typeof e.content === 'string' &&
    Array.isArray(e.tags) &&
    e.tags.every((t) => Array.isArray(t) && t.every((v) => typeof v === 'string'))
  )
}

/** True if the event's `id` is its hash and its `sig` verifies. */
export function verifySignedEvent(event: SignedEvent): boolean {
  try {
    // Rebuild from the seven NIP-01 fields only. nostr-tools caches a
    // "verified" flag under a symbol on events it signed or checked, and an
    // object spread would carry that flag across a tampered copy.
    const bare: NostrEvent = {
      id: event.id,
      pubkey: event.pubkey,
      created_at: event.created_at,
      kind: event.kind,
      tags: event.tags,
      content: event.content,
      sig: event.sig,
    }
    return verifyEvent(bare)
  } catch {
    return false
  }
}

export function hasMarker(event: SignedEvent): boolean {
  return event.tags.some((t) => t.length >= 2 && t[0] === 't' && t[1] === 'charter-device')
}

function tagValues(event: SignedEvent, name: string): string[] {
  return event.tags.filter((t) => t[0] === name).map((t) => t[1] ?? '')
}

/**
 * Parse and validate a guardian statement on its own terms.
 *
 * Checks run in this order, and the first failure is the reason: event shape,
 * signature, kind, marker, `d` count, `p` count, `d` prefix, key format,
 * cancel/objection target, `d`/`p` agreement, content strict JSON, `v`, `issuedAt`,
 * `label`, `expiresAt`.
 */
export function parseStatement(event: unknown): ParseStatementResult {
  if (!isEventShape(event)) return { ok: false, reason: 'bad_event' }
  if (!verifySignedEvent(event)) return { ok: false, reason: 'bad_signature' }
  if (event.kind !== CHARTER_GUARDIAN_STATEMENT) return { ok: false, reason: 'wrong_kind' }
  if (!hasMarker(event)) return { ok: false, reason: 'missing_marker' }
  const ds = tagValues(event, 'd')
  if (ds.length !== 1) return { ok: false, reason: 'd_tag_count' }
  const ps = tagValues(event, 'p')
  if (ps.length !== 1) return { ok: false, reason: 'p_tag_count' }
  const d = ds[0] as string
  const p = ps[0] as string
  const split = splitD(d)
  if (!split) return { ok: false, reason: 'unknown_d_prefix' }
  if (!isHex64(p)) return { ok: false, reason: 'bad_key' }

  let key: string
  let target: string | null = null
  let targetType: PendingType | null = null
  if (SIMPLE_TYPES.includes(split.type)) {
    if (!isHex64(split.rest)) return { ok: false, reason: 'bad_key' }
    key = split.rest
    if (key !== p) return { ok: false, reason: 'd_p_mismatch' }
  } else if (split.type === 'cancel') {
    // A cancel names a pending co-guardian, removal or move; nothing else is
    // ever pending. See open question "Cancel targets".
    const inner = splitPendingD(split.rest)
    if (!inner) return { ok: false, reason: 'bad_cancel_target' }
    key = inner.key
    target = split.rest
    targetType = inner.type
    if (key !== p) return { ok: false, reason: 'd_p_mismatch' }
  } else {
    // objection: d = kintrinsic/object/<removal d>, p = the removal's signer.
    const inner = splitPendingD(split.rest)
    if (!inner || inner.type !== 'removal') return { ok: false, reason: 'bad_objection_target' }
    key = inner.key
    target = split.rest
    targetType = 'removal'
  }

  // Strict JSON: no duplicate names, integer number tokens only, no lone
  // surrogates, so every ward reads one event the same way.
  const parsedContent = tryParseStrictJson(event.content)
  if (!parsedContent.ok) return { ok: false, reason: 'content_not_json' }
  const content = parsedContent.value
  if (typeof content !== 'object' || content === null || Array.isArray(content)) {
    return { ok: false, reason: 'content_not_json' }
  }
  const c = content as Record<string, unknown>
  if (c.v !== 1) return { ok: false, reason: 'bad_version' }
  if (!isUnixSeconds(c.issuedAt)) return { ok: false, reason: 'bad_issued_at' }
  let label: string | null = null
  if (c.label !== undefined) {
    if (typeof c.label !== 'string' || utf8Length(c.label) > MAX_LABEL_BYTES) {
      return { ok: false, reason: 'bad_label' }
    }
    label = c.label
  }
  let expiresAt: number | null = null
  if (c.expiresAt !== undefined) {
    if (!isUnixSeconds(c.expiresAt)) return { ok: false, reason: 'bad_expires_at' }
    expiresAt = c.expiresAt
  }

  return {
    ok: true,
    statement: {
      type: split.type,
      d,
      p,
      key,
      target,
      targetType,
      signer: event.pubkey,
      id: event.id,
      issuedAt: c.issuedAt,
      label,
      expiresAt,
      event,
    },
  }
}
