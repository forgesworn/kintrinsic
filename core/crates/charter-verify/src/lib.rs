//! `charter-verify` — the trust-critical pure verification crate. The only way
//! an enactor can act is a [`VerifiedGrant`] (no public constructor) returned by
//! [`verify_grant`], and the only way a standing clause is cached is a
//! [`VerifiedClause`] returned by [`verify_clause`]. Both authenticate against
//! the **pinned guardian** over the single `charter-crypto` BIP-340 backend.

pub mod clause;
pub mod event;
pub mod release;
pub mod software_release;
pub mod usage_sync;
pub mod verify;

#[cfg(feature = "mock")]
pub mod test_support;

/// Re-export of the single crypto backend (no second backend anywhere).
pub use charter_crypto as crypto;

pub use clause::{verify_clause, ClauseError, VerifiedClause};
pub use event::{event_id, id_is_consistent, signature_is_valid};
pub use release::{verify_release, ReleaseError, RELEASE_FRESHNESS_SECS};
pub use software_release::{verify_software_release, SoftwareRelease, SoftwareReleaseError};
pub use usage_sync::{verify_usage_sync, UsageSyncError, VerifiedUsageSync};
pub use verify::{
    verify_grant, GrantOutcome, VerifiedGrant, VerifyError, VerifyParams, FRESHNESS_SKEW_SECS,
};
