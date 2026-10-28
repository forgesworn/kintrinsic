/**
 * Kinds, limits and shared types for *Guardian roots and working keys (v1)*
 * (`spec/contract.md`). Every value here is fixed by the contract; the
 * comments name the clause it comes from.
 */

/** Scan-to-pair offer, guardian → unpaired ward (existing kind). */
export const CHARTER_DEVICE_PAIR_OFFER = 31117
/** Family roster, guardian install ↔ guardian install. Wards never read it. */
export const CHARTER_FAMILY_ROSTER = 31119
/** Guardian statement (approval, withdrawal, co-guardian, …), addressable. */
export const CHARTER_GUARDIAN_STATEMENT = 31120
/** Presence confirmation, a working key at the ward → the ward. */
export const CHARTER_PRESENCE_CONFIRM = 31121

/** The marker tag every Charter inner event carries. */
export const CHARTER_MARKER_TAG: readonly [string, string] = ['t', 'charter-device']

/** At most 4 roots per ward. */
export const MAX_ROOTS = 4
/** At most 16 active working keys per root. */
export const MAX_ACTIVE_KEYS_PER_ROOT = 16
/** Each root's tombstone quota (rule 10). */
export const MAX_TOMBSTONES_PER_ROOT = 256
/**
 * Each root's quota of held early statements (rule 10 names withdrawals; this
 * reference also counts held cancels and objections against it, see
 * docs/guardian-keys-open-questions.md, "Held-statement quota").
 */
export const MAX_HELD_PER_ROOT = 64
/** A CLAUSE or statement more than this far ahead of the ward clock is refused. */
export const FUTURE_SKEW_SECONDS = 300
/** Pending statements expire at most this long after `issuedAt`. */
export const PENDING_MAX_SECONDS = 7 * 24 * 60 * 60
/** Notice period for a root removal signed by another root (rule 7). */
export const REMOVAL_NOTICE_SECONDS = 72 * 60 * 60
/**
 * An objection blocks further removals of its signer by the same remover for
 * this long after it reaches the ward. OQ: Removal re-issued after an objection.
 */
export const OBJECTION_BLOCK_SECONDS = 7 * 24 * 60 * 60
/** The existing RELAYS rule: an event more than this old is refused. */
export const RELAYS_TTL_SECONDS = 600
/** Presence and pairing token lifetime. */
export const TOKEN_LIFETIME_SECONDS = 600
/** After a RELAYS change, a multi-root ward also sends STATUS on the old relays. */
export const PREVIOUS_RELAYS_SECONDS = 7 * 24 * 60 * 60
/** Maximum label length, in UTF-8 bytes. */
export const MAX_LABEL_BYTES = 64

/** A signed NIP-01 event. */
export interface SignedEvent {
  id: string
  pubkey: string
  created_at: number
  kind: number
  tags: string[][]
  content: string
  sig: string
}

/** An unsigned NIP-01 event template, as handed to a signer. */
export interface EventTemplate {
  kind: number
  created_at: number
  tags: string[][]
  content: string
}

/**
 * What a ward has after opening a NIP-59 gift wrap: the seal's authenticated
 * author and the signed inner event.
 */
export interface Inbound {
  sealAuthor: string
  inner: SignedEvent
}

export const HEX64 = /^[0-9a-f]{64}$/
export const HEX32 = /^[0-9a-f]{32}$/

export function isHex64(x: unknown): x is string {
  return typeof x === 'string' && HEX64.test(x)
}

/** Unix seconds as the contract uses them: a non-negative safe integer. */
export function isUnixSeconds(x: unknown): x is number {
  return typeof x === 'number' && Number.isSafeInteger(x) && x >= 0
}

export function utf8Length(s: string): number {
  return new TextEncoder().encode(s).length
}

/** Constant-time comparison of two equal-length strings. */
export function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false
  let diff = 0
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i)
  return diff === 0
}
