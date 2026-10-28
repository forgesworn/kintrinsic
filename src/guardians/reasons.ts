/**
 * Stable reason codes for *Guardian roots and working keys (v1)*.
 *
 * These strings are part of the golden vectors
 * (`core/crates/charter-testkit/vectors/guardians/`): the Rust and Android
 * wards compare against them, so a code is never renamed or reused for a
 * different meaning. The vector README lists each with its meaning.
 */

/** Why a statement event (kind 31120) is malformed. */
export const STATEMENT_REASONS = [
  'bad_event', // not a well-formed NIP-01 event object
  'bad_signature', // `id` is not the event hash, or `sig` does not verify
  'wrong_kind', // kind is not 31120
  'missing_marker', // no ["t","charter-device"] tag
  'd_tag_count', // not exactly one `d` tag
  'p_tag_count', // not exactly one `p` tag
  'unknown_d_prefix', // `d` does not start with a known kintrinsic/ prefix
  'bad_key', // the key in `d` or `p` is not 64 lowercase hex
  'd_p_mismatch', // `d` and `p` disagree
  'bad_cancel_target', // a cancel's `d` does not name a co-guardian, removal or move
  'bad_objection_target', // an objection's `d` does not name a removal
  'content_not_json', // content is not a JSON object
  'bad_version', // `v` is not the number 1
  'bad_issued_at', // `issuedAt` missing or not a non-negative integer
  'bad_label', // `label` not a string, or over 64 UTF-8 bytes
  'bad_expires_at', // `expiresAt` present but not a non-negative integer
] as const
export type StatementReason = (typeof STATEMENT_REASONS)[number]

/** Why a ward accepted an input. */
export const ACCEPT_REASONS = [
  'paired',
  'approval_applied',
  'approval_updated',
  'approval_rebound',
  'approval_preempted',
  'withdrawal_applied',
  'withdrawal_held',
  'coguardian_pending',
  'removal_pending',
  'removal_applied',
  'move_pending',
  'cancel_applied',
  'cancel_held',
  'objection_applied',
  'objection_held',
  'token_registered',
  'coguardian_applied',
  'move_applied',
  'removal_confirmed',
  'clause_accepted',
  'signed_accepted',
  'relays_applied',
  'status_sent',
  'ticked',
] as const
export type AcceptReason = (typeof ACCEPT_REASONS)[number]

/** Why a ward dropped an input (beyond a malformed statement). */
export const DROP_REASONS = [
  'future',
  'seal_mismatch',
  'signer_not_pinned',
  'not_signer',
  'duplicate',
  'stale',
  'tie_not_newer',
  'expires_at_not_allowed',
  'key_role_conflict',
  'key_departed',
  'bound_elsewhere',
  'root_at_quota',
  'root_key_quota',
  'held_quota',
  'not_binding_root',
  'already_withdrawn',
  'already_pinned',
  'target_not_pinned',
  'not_newer_than_departure',
  'root_quota',
  'last_root',
  'expired',
  'cancelled',
  'objected',
  'cancel_signer_mismatch',
  'objection_mismatch',
  'not_pending',
  'bad_confirm',
  'token_invalid',
  'confirmer_not_bound',
  'approval_required',
  'bad_approval',
  'pairing_floor',
  'not_newer',
  'bad_offer',
  'pin_not_wrapped',
  'bad_pin_record',
] as const
export type DropReason = (typeof DROP_REASONS)[number]

export type ReasonCode = StatementReason | AcceptReason | DropReason

/** What happened to a pending statement during a `tick`. */
export const TICK_RESULTS = ['applied', 'expired', 'refused'] as const
export type TickResult = (typeof TICK_RESULTS)[number]
