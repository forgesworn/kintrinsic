//! `verify_clause` — authenticate a standing schedule/budget clause against the
//! inner NIP-01 signature AND an author in the ward's signer set, with per-kind
//! monotonic `issuedAt` rollback protection. A hostile relay can delay/drop a clause but
//! can never forge one or roll it back. The body is left opaque (Phase 6 parses
//! the schedule/budget shape).

use charter_primitives::{NostrEvent, PubKey, SignerSet};
use charter_proto::{ClauseKind, ClausePayload};

use crate::event::{event_id, signature_is_valid};

// M5 NOTE: a clause freshness/age bound is intentionally NOT enforced here. The
// design is fail-safe-offline — a managed device may run with a lagging/leading
// clock (dead RTC, pre-NTP boot), so a `now`-based skew gate would wrongly reject
// legitimately-signed guardian clauses and silently block tightening/revocation.
// A clause dated AHEAD of the ward's clock is not refused either (contract, *Who
// may sign what*, D1): a guardian-keys ward clamps it to its own clock — the
// slot's bar becomes min(issuedAt, now at acceptance), never below the bar it
// had, and each signer's highest accepted issuedAt for the slot is its replay
// floor — in `charter-guardians`'
// `accept_clause`, which the spine's clause store defers to. A far-future
// clause therefore pins nothing, and a guardian whose phone runs fast is never
// locked out. Rollback is covered by the durable per-kind monotonic `issuedAt`
// (the precise floor after the first clause); the first-seen replay window is
// low risk (no-clause = unlocked-by-default, and the guardian's real clause
// always wins on `issuedAt`). A durable issuedAt floor is the complete fix
// (deferred).

/// An authenticated clause. Fields are private; only [`verify_clause`] builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedClause {
    kind: ClauseKind,
    issued_at: u64,
    signer: PubKey,
    body: serde_json::Value,
}

impl VerifiedClause {
    /// The clause type.
    pub fn kind(&self) -> ClauseKind {
        self.kind
    }
    /// The clause `issuedAt` (drives rollback protection).
    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }
    /// Who signed it: a key in the signer set (rule 5 records it with the
    /// stored clause).
    pub fn signer(&self) -> PubKey {
        self.signer
    }
    /// The opaque clause body (Phase 6 parses schedule/budget).
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
}

/// Why a clause was rejected. Every `Err` means "do not cache / enforce this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClauseError {
    /// Wrong kind, unparseable payload.
    Malformed,
    /// Event-id integrity or schnorr signature failed.
    BadSignature,
    /// The author is not in the signer set.
    UntrustedSigner,
    /// `issuedAt` was at or below the highest already seen (rollback).
    StaleIssuedAt,
    /// The payload kind did not match the expected kind.
    BadShape,
}

/// Authenticate a CLAUSE event. `prev_issued_at` is the highest `issuedAt`
/// previously accepted for `kind` (None if never). Order: malformed → kind
/// match → event-id integrity → signature → author in the signer set →
/// monotonic issuedAt. The floor is checked last, so a
/// [`ClauseError::StaleIssuedAt`] clause is otherwise authentic. `now` is
/// unused: no skew gate (see the M5 note).
pub fn verify_clause(
    event: &NostrEvent,
    signers: &dyn SignerSet,
    kind: ClauseKind,
    prev_issued_at: Option<u64>,
    _now: u64,
) -> Result<VerifiedClause, ClauseError> {
    if event.kind != charter_primitives::kinds::CHARTER_DEVICE_CLAUSE {
        return Err(ClauseError::Malformed);
    }
    let payload = ClausePayload::from_json(&event.content).map_err(|_| ClauseError::Malformed)?;

    if payload.kind != kind {
        return Err(ClauseError::BadShape);
    }

    if event_id(event) != event.id {
        return Err(ClauseError::BadSignature);
    }
    if !signature_is_valid(event) {
        return Err(ClauseError::BadSignature);
    }
    if !signers.is_signer(&event.pubkey) {
        return Err(ClauseError::UntrustedSigner);
    }
    // Monotonic rollback protection (the precise floor once a clause is cached).
    if let Some(prev) = prev_issued_at {
        if payload.issued_at <= prev {
            return Err(ClauseError::StaleIssuedAt);
        }
    }

    Ok(VerifiedClause {
        kind: payload.kind,
        issued_at: payload.issued_at,
        signer: event.pubkey,
        body: payload.body,
    })
}
