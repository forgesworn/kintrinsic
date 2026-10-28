//! `verify_usage_sync` — authenticate a consolidated cross-device usage view
//! (kind 31115) against the pinned guardian's inner NIP-01 signature AND
//! author, with per-subject monotonic `ts` replay protection. Usage-sync only
//! ever *shrinks* what a device believes remains, and staleness only
//! under-counts the pool, so a hostile relay can delay/drop a sync but can
//! never forge one, roll one back, or cause a wrongful early lock.

use charter_primitives::{NostrEvent, SignerSet};
use charter_proto::UsageSyncPayload;
use charter_schedule::MinuteSet;

use crate::event::{event_id, signature_is_valid};
use crate::verify::FRESHNESS_SKEW_SECS;

/// An authenticated usage-sync. Fields are private; only
/// [`verify_usage_sync`] builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUsageSync {
    ts: u64,
    payload: UsageSyncPayload,
}

impl VerifiedUsageSync {
    /// The guardian timestamp (drives per-subject replay protection).
    pub fn ts(&self) -> u64 {
        self.ts
    }
    /// The consolidated view.
    pub fn payload(&self) -> &UsageSyncPayload {
        &self.payload
    }
}

/// Why a usage-sync was rejected. Every `Err` means "do not store / use this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageSyncError {
    /// Wrong kind, unparseable payload, or a malformed minute bitmap.
    Malformed,
    /// Event-id integrity or schnorr signature failed.
    BadSignature,
    /// The author is not in the signer set.
    UntrustedSigner,
    /// `ts` is more than [`FRESHNESS_SKEW_SECS`] ahead of the ward's clock.
    FutureTs,
    /// `ts` was at or below the highest already seen for this subject (replay).
    StaleTs,
}

/// Authenticate a USAGE_SYNC event. `prev_ts` is the highest `ts` previously
/// accepted for this subject (None if never). Order mirrors `verify_clause`:
/// malformed (incl. the bitmap, fail-closed) → event-id integrity → signature
/// → author in the signer set → not future-dated → monotonic ts.
pub fn verify_usage_sync(
    event: &NostrEvent,
    signers: &dyn SignerSet,
    prev_ts: Option<u64>,
    now: u64,
) -> Result<VerifiedUsageSync, UsageSyncError> {
    if event.kind != charter_primitives::kinds::CHARTER_DEVICE_USAGE_SYNC {
        return Err(UsageSyncError::Malformed);
    }
    let payload =
        UsageSyncPayload::from_json(&event.content).map_err(|_| UsageSyncError::Malformed)?;

    // A present-but-undecodable bitmap is a malformed payload, not "scalar
    // fallback" — fail closed at the trust boundary.
    if let Some(bm) = &payload.elsewhere_minutes_today {
        if MinuteSet::from_b64url(bm).is_none() {
            return Err(UsageSyncError::Malformed);
        }
    }

    if event_id(event) != event.id {
        return Err(UsageSyncError::BadSignature);
    }
    if !signature_is_valid(event) {
        return Err(UsageSyncError::BadSignature);
    }
    if !signers.is_signer(&event.pubkey) {
        return Err(UsageSyncError::UntrustedSigner);
    }
    if payload.ts > now.saturating_add(FRESHNESS_SKEW_SECS) {
        return Err(UsageSyncError::FutureTs);
    }

    if let Some(prev) = prev_ts {
        if payload.ts <= prev {
            return Err(UsageSyncError::StaleTs);
        }
    }

    Ok(VerifiedUsageSync {
        ts: payload.ts,
        payload,
    })
}
