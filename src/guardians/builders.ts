/**
 * Guardian-side builders for *Guardian roots and working keys (v1)*.
 *
 * Each `*Template` function returns an unsigned NIP-01 event template; sign
 * it with `signTemplate`, which takes either a secret key or any signer
 * function (so a NIP-46 remote signer, My Signet or a Heartwood works the
 * same way as a local key). The `build*` helpers do both in one call.
 *
 * `created_at` defaults to the statement's `issuedAt` and is never read from
 * a clock, so the same inputs always give the same template.
 */
import { finalizeEvent } from 'nostr-tools/pure'
import { STATEMENT_PREFIXES, splitPendingD } from './statements.js'
import {
  CHARTER_DEVICE_PAIR_OFFER,
  CHARTER_FAMILY_ROSTER,
  CHARTER_GUARDIAN_STATEMENT,
  CHARTER_MARKER_TAG,
  CHARTER_PRESENCE_CONFIRM,
  HEX32,
  MAX_LABEL_BYTES,
  isHex64,
  isUnixSeconds,
  utf8Length,
  type EventTemplate,
  type SignedEvent,
} from './types.js'

/** Anything that can sign a template: a local key, a NIP-46 bunker, … */
export type EventSigner = (template: EventTemplate) => SignedEvent | Promise<SignedEvent>

/** A signer backed by a local secret key. */
export function secretKeySigner(secretKey: Uint8Array): EventSigner {
  return (template) => finalizeEvent({ ...template, tags: template.tags.map((t) => [...t]) }, secretKey) as SignedEvent
}

/** Sign a template with a secret key or a signer function. */
export async function signTemplate(template: EventTemplate, signer: EventSigner | Uint8Array): Promise<SignedEvent> {
  const sign = signer instanceof Uint8Array ? secretKeySigner(signer) : signer
  return await sign(template)
}

export interface StatementOptions {
  issuedAt: number
  /** A display label, at most 64 UTF-8 bytes. */
  label?: string
  /** Pending statements only (co-guardian, removal, move). Clamped by wards to 7 days. */
  expiresAt?: number
  /** Defaults to `issuedAt`. Wards ignore it. */
  createdAt?: number
}

function requireHex64(name: string, v: string): void {
  if (!isHex64(v)) throw new Error(`${name} must be 64 lowercase hex`)
}

function statementTemplate(d: string, p: string, opts: StatementOptions, allowExpiry: boolean): EventTemplate {
  if (!isUnixSeconds(opts.issuedAt)) throw new Error('issuedAt must be Unix seconds')
  const content: Record<string, unknown> = { v: 1, issuedAt: opts.issuedAt }
  if (opts.label !== undefined) {
    if (utf8Length(opts.label) > MAX_LABEL_BYTES) throw new Error(`label exceeds ${MAX_LABEL_BYTES} UTF-8 bytes`)
    content.label = opts.label
  }
  if (opts.expiresAt !== undefined) {
    if (!allowExpiry) throw new Error('expiresAt applies only to co-guardian, removal and move statements')
    if (!isUnixSeconds(opts.expiresAt)) throw new Error('expiresAt must be Unix seconds')
    content.expiresAt = opts.expiresAt
  }
  return {
    kind: CHARTER_GUARDIAN_STATEMENT,
    created_at: opts.createdAt ?? opts.issuedAt,
    tags: [['d', d], ['p', p], [...CHARTER_MARKER_TAG]],
    content: JSON.stringify(content),
  }
}

/** Approval (signed by R): working key W joins the signer set, bound to R. */
export function approvalTemplate(workingKey: string, opts: StatementOptions): EventTemplate {
  requireHex64('workingKey', workingKey)
  return statementTemplate(STATEMENT_PREFIXES.approval + workingKey, workingKey, opts, false)
}

/** Withdrawal (signed by W's binding root): W leaves the signer set for good. */
export function withdrawalTemplate(workingKey: string, opts: StatementOptions): EventTemplate {
  requireHex64('workingKey', workingKey)
  return statementTemplate(STATEMENT_PREFIXES.withdrawal + workingKey, workingKey, opts, false)
}

/** Co-guardian (signed by a pinned root): R2 is pinned after presence confirmation. */
export function coGuardianTemplate(newRoot: string, opts: StatementOptions): EventTemplate {
  requireHex64('newRoot', newRoot)
  return statementTemplate(STATEMENT_PREFIXES.coguardian + newRoot, newRoot, opts, true)
}

/** Root removal (signed by R2 itself, or by another pinned root). */
export function rootRemovalTemplate(removedRoot: string, opts: StatementOptions): EventTemplate {
  requireHex64('removedRoot', removedRoot)
  return statementTemplate(STATEMENT_PREFIXES.removal + removedRoot, removedRoot, opts, true)
}

/** Root move (signed by the root being replaced): R2 replaces it after presence confirmation. */
export function rootMoveTemplate(newRoot: string, opts: StatementOptions): EventTemplate {
  requireHex64('newRoot', newRoot)
  return statementTemplate(STATEMENT_PREFIXES.move + newRoot, newRoot, opts, true)
}

/**
 * Cancel (signed by the pending statement's signer): drops the pending
 * co-guardian, removal or move with `pendingD` at or below this `issuedAt`.
 */
export function cancelTemplate(pendingD: string, opts: StatementOptions): EventTemplate {
  const target = splitPendingD(pendingD)
  if (!target) throw new Error('pendingD must be a co-guardian, removal or move d')
  return statementTemplate(STATEMENT_PREFIXES.cancel + pendingD, target.key, opts, false)
}

/**
 * Objection (signed by the root being removed): drops the pending removal
 * `removalD` signed by `removalSigner`.
 */
export function objectionTemplate(removalD: string, removalSigner: string, opts: StatementOptions): EventTemplate {
  const target = splitPendingD(removalD)
  if (!target || target.type !== 'removal') throw new Error('removalD must be a removal d')
  requireHex64('removalSigner', removalSigner)
  return statementTemplate(STATEMENT_PREFIXES.objection + removalD, removalSigner, opts, false)
}

export interface PresenceConfirmOptions {
  /** The 32-hex token scanned or typed from the ward's screen. */
  token: string
  /** The pending statement's event id. */
  statement: string
  /** For a co-guardian or move: R2's signed approval of the confirming W. */
  approval?: SignedEvent
  createdAt: number
}

/** Presence confirmation (kind 31121), signed and sealed by the confirming W. */
export function presenceConfirmTemplate(opts: PresenceConfirmOptions): EventTemplate {
  if (!HEX32.test(opts.token)) throw new Error('token must be 32 lowercase hex')
  requireHex64('statement', opts.statement)
  const content: Record<string, unknown> = { v: 1, token: opts.token, statement: opts.statement }
  if (opts.approval) content.approval = opts.approval
  return {
    kind: CHARTER_PRESENCE_CONFIRM,
    created_at: opts.createdAt,
    tags: [[...CHARTER_MARKER_TAG]],
    content: JSON.stringify(content),
  }
}

export interface RosterDevice {
  machine: string
  platform: string
  name: string
}

export interface RosterChild {
  subject: string
  issuedAt: number
  name: string
  colour: string
  devices: RosterDevice[]
}

export interface FamilyRosterOptions {
  children: RosterChild[]
  /** The sender's own approval, signed by its root. */
  approval: SignedEvent
  createdAt: number
}

/**
 * Family roster (kind 31119), signed by a W and gift-wrapped to each other
 * guardian working key. The sender's approval travels in the content as
 * `approval`. OQ: Where the roster carries its approval.
 */
export function familyRosterTemplate(opts: FamilyRosterOptions): EventTemplate {
  return {
    kind: CHARTER_FAMILY_ROSTER,
    created_at: opts.createdAt,
    tags: [[...CHARTER_MARKER_TAG]],
    content: JSON.stringify({ v: 1, children: opts.children, approval: opts.approval }),
  }
}

export interface PairOfferOptions {
  token: string
  relays: string[]
  ts: number
  /** When the offer is sealed by a working key: its approval, signed by R. */
  approval?: SignedEvent
}

/** The PAIR_OFFER (kind 31117) content, with the working key's approval when sealed by W. */
export function pairOfferContent(opts: PairOfferOptions): string {
  if (!HEX32.test(opts.token)) throw new Error('token must be 32 lowercase hex')
  const payload: Record<string, unknown> = { token: opts.token, relays: opts.relays, ts: opts.ts }
  if (opts.approval) payload.approval = opts.approval
  return JSON.stringify(payload)
}

/** A PAIR_OFFER inner event template. */
export function pairOfferTemplate(opts: PairOfferOptions & { createdAt?: number }): EventTemplate {
  return {
    kind: CHARTER_DEVICE_PAIR_OFFER,
    created_at: opts.createdAt ?? opts.ts,
    tags: [[...CHARTER_MARKER_TAG]],
    content: pairOfferContent(opts),
  }
}

// One-call helpers: build and sign.

export const buildApproval = (signer: EventSigner | Uint8Array, workingKey: string, opts: StatementOptions) =>
  signTemplate(approvalTemplate(workingKey, opts), signer)
export const buildWithdrawal = (signer: EventSigner | Uint8Array, workingKey: string, opts: StatementOptions) =>
  signTemplate(withdrawalTemplate(workingKey, opts), signer)
export const buildCoGuardian = (signer: EventSigner | Uint8Array, newRoot: string, opts: StatementOptions) =>
  signTemplate(coGuardianTemplate(newRoot, opts), signer)
export const buildRootRemoval = (signer: EventSigner | Uint8Array, removedRoot: string, opts: StatementOptions) =>
  signTemplate(rootRemovalTemplate(removedRoot, opts), signer)
export const buildRootMove = (signer: EventSigner | Uint8Array, newRoot: string, opts: StatementOptions) =>
  signTemplate(rootMoveTemplate(newRoot, opts), signer)
export const buildCancel = (signer: EventSigner | Uint8Array, pendingD: string, opts: StatementOptions) =>
  signTemplate(cancelTemplate(pendingD, opts), signer)
export const buildObjection = (
  signer: EventSigner | Uint8Array,
  removalD: string,
  removalSigner: string,
  opts: StatementOptions,
) => signTemplate(objectionTemplate(removalD, removalSigner, opts), signer)
export const buildPresenceConfirm = (signer: EventSigner | Uint8Array, opts: PresenceConfirmOptions) =>
  signTemplate(presenceConfirmTemplate(opts), signer)
export const buildFamilyRoster = (signer: EventSigner | Uint8Array, opts: FamilyRosterOptions) =>
  signTemplate(familyRosterTemplate(opts), signer)
