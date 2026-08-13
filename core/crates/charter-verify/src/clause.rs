//! `verify_clause` — authenticate a standing schedule/budget clause against the
//! pinned guardian's inner NIP-01 signature AND author, with per-kind monotonic
//! `issuedAt` rollback protection. A hostile relay can delay/drop a clause but
//! can never forge one or roll it back. The body is left opaque (Phase 6 parses
//! the schedule/budget shape).

use charter_primitives::{NostrEvent, PubKey};
use charter_proto::{ClauseKind, ClausePayload};

use crate::event::{event_id, signature_is_valid};

// M5 NOTE: a clause freshness/age bound is intentionally NOT enforced here. The
// design is fail-safe-offline — a managed device may run with a lagging/leading
// clock (dead RTC, pre-NTP boot), so a `now`-based skew gate would wrongly reject
// legitimately-signed guardian clauses and silently block tightening/revocation.
// Rollback is covered by the durable per-kind monotonic `issuedAt` (the precise
// floor after the first clause); the first-seen replay window is low risk
// (no-clause = unlocked-by-default, and the guardian's real clause always wins on
// `issuedAt`). A durable issuedAt floor is the complete fix (deferred).

/// An authenticated clause. Fields are private; only [`verify_clause`] builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedClause {
    kind: ClauseKind,
    issued_at: u64,
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
    /// The author is not the pinned guardian.
    UntrustedSigner,
    /// `issuedAt` was at or below the highest already seen (rollback).
    StaleIssuedAt,
    /// The payload kind did not match the expected kind.
    BadShape,
}

/// Authenticate a CLAUSE event. `prev_issued_at` is the highest `issuedAt`
/// previously accepted for `kind` (None if never). Order: malformed → kind
/// match → event-id integrity → signature → pinned author → monotonic issuedAt.
pub fn verify_clause(
    event: &NostrEvent,
    pinned_guardian: &PubKey,
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
    if &event.pubkey != pinned_guardian {
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
        body: payload.body,
    })
}
