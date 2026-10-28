//! `charter-guardians` — the ward side of *Guardian roots and working keys
//! (v1)* (`spec/contract.md`): who may set rules on a child's device.
//!
//! A pure state machine, ported from the reference in `src/guardians/` and
//! held to its golden vectors in `charter-testkit/vectors/guardians/`:
//! statement validation (strict JSON content, NIP-01 id and BIP-340
//! signature), the signer set, working-key bindings, pending co-guardian,
//! removal and move statements, presence confirmation, departures and the
//! clause floor, RELAYS with the guardian PIN, scan-to-pair with a working
//! key, recipients and the STATUS `guardians` projection.
//!
//! No clock, no IO, no randomness: every entry point takes `now` (Unix
//! seconds) and returns a new state with an [`Outcome`]. Signed events are
//! taken as `serde_json::Value` (`serde_json::to_value(&nostr_event)` for a
//! ward holding a typed event) so a malformed one is refused with the right
//! reason rather than failing to deserialise.

pub mod event;
pub mod json;
pub mod pin;
pub mod reason;
pub mod state;
pub mod statement;

pub use json::{parse_strict_json, StrictJsonError};
pub use pin::{
    pin_backoff_seconds, pin_hash, pin_next_try_at, validate_pin_record, verify_pin,
    GuardianPinRecord,
};
pub use reason::{Reason, TickResult, ACCEPT_REASONS, DROP_REASONS, STATEMENT_REASONS};
pub use state::{
    accept_clause, accept_signed_event, apply_inbound, apply_presence_confirm, apply_relays,
    held_clause, is_departed, is_pinned, is_signer, mark_status_sent, pair_from_offer,
    pending_expiry, project_state, recipients, register_presence_token, root_of, signer_set,
    status_guardians, status_recipients, status_relays, tick, ClauseInput, GuardianState, Outcome,
    PairToken, Paired, ProjectedState, RelaysChannel, RelaysInput, StatusGuardians, Step,
    TickEntry, Verdict,
};
pub use statement::{parse_statement, split_pending_d, PendingType, Statement, StatementType};

/// Scan-to-pair offer, guardian → unpaired ward.
pub const CHARTER_DEVICE_PAIR_OFFER: u16 = 31117;
/// Family roster, guardian install ↔ guardian install. Wards never read it.
pub const CHARTER_FAMILY_ROSTER: u16 = 31119;
/// Guardian statement (approval, withdrawal, co-guardian, …), addressable.
pub const CHARTER_GUARDIAN_STATEMENT: u16 = 31120;
/// Presence confirmation, a working key at the ward → the ward.
pub const CHARTER_PRESENCE_CONFIRM: u16 = 31121;

/// At most 4 roots per ward.
pub const MAX_ROOTS: usize = 4;
/// At most 16 active working keys per root.
pub const MAX_ACTIVE_KEYS_PER_ROOT: usize = 16;
/// Each root's tombstone quota (rule 10).
pub const MAX_TOMBSTONES_PER_ROOT: usize = 256;
/// Each root's quota of held early statements: withdrawals, and also held
/// cancels and objections. OQ: Held-statement quota.
pub const MAX_HELD_PER_ROOT: usize = 64;
/// A CLAUSE or statement more than this far ahead of the ward clock is refused.
pub const FUTURE_SKEW_SECONDS: i64 = 300;
/// Pending statements expire at most this long after `issuedAt`.
pub const PENDING_MAX_SECONDS: i64 = 7 * 24 * 60 * 60;
/// Notice period for a root removal signed by another root (rule 7).
pub const REMOVAL_NOTICE_SECONDS: i64 = 72 * 60 * 60;
/// An objection blocks further removals of its signer by the same remover
/// for this long after it reaches the ward. OQ: Removal re-issued after an
/// objection.
pub const OBJECTION_BLOCK_SECONDS: i64 = 7 * 24 * 60 * 60;
/// The existing RELAYS rule: an event more than this old is refused.
pub const RELAYS_TTL_SECONDS: i64 = 600;
/// Presence and pairing token lifetime.
pub const TOKEN_LIFETIME_SECONDS: i64 = 600;
/// After a RELAYS change, a multi-root ward also sends STATUS on the old
/// relays for this long.
pub const PREVIOUS_RELAYS_SECONDS: i64 = 7 * 24 * 60 * 60;
/// At most this many replaced relay lists are remembered; the oldest goes.
pub const MAX_PREVIOUS_RELAYS: usize = 8;
/// Maximum label length, in UTF-8 bytes.
pub const MAX_LABEL_BYTES: usize = 64;
