//! `charter-link` — the local link (v1): direct delivery between a paired
//! guardian carrier and its wards on the same home network.
//!
//! The link only changes how a NIP-59 gift wrap travels. Every wrap it
//! carries still goes through the same unwrap and verification as its relay
//! copy; nothing here grants authority. See `spec/contract.md`, "Local link
//! (v1)", and `docs/local-network-design.md`.

use charter_primitives::NostrEvent;
use serde::{Deserialize, Serialize};

pub mod proto;
pub use proto::*;

#[cfg(feature = "net")]
mod guardian;
#[cfg(feature = "net")]
mod session;
#[cfg(feature = "net")]
mod store;
#[cfg(feature = "net")]
mod ward;

#[cfg(feature = "net")]
pub use guardian::{
    DeliveryRecord, DeliveryState, GuardianEvent, GuardianLink, GuardianLinkConfig, InboxEntry,
    SendState, SessionInfo,
};
#[cfg(feature = "net")]
pub use ward::{
    PrepareSocket, WardIngest, WardLink, WardLinkConfig, WardLinkSnapshot, WardSessionInfo,
};

/// What a ward reports, in a signed ACK, for a guardian wrap it received over
/// the local link. Serialised in lowercase on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AckResult {
    /// Verified, and this delivery made it the device's state.
    Applied,
    /// Verified, and the device already held it from an earlier copy.
    Unchanged,
    /// An authentic clause older than the one in effect; the newer one stays.
    Superseded,
    /// A grant past its window, or for an ask that already expired or closed.
    Expired,
    /// Failed verification or validation.
    Refused,
    /// Authentic, but the device could not record or enact it.
    Failed,
    /// A kind the device does not act on over the local link.
    Ignored,
}

impl AckResult {
    /// The wire spelling, also used in the ACK signature digest.
    pub fn as_str(self) -> &'static str {
        match self {
            AckResult::Applied => "applied",
            AckResult::Unchanged => "unchanged",
            AckResult::Superseded => "superseded",
            AckResult::Expired => "expired",
            AckResult::Refused => "refused",
            AckResult::Failed => "failed",
            AckResult::Ignored => "ignored",
        }
    }

    /// Parse the wire spelling; anything else is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "applied" => AckResult::Applied,
            "unchanged" => AckResult::Unchanged,
            "superseded" => AckResult::Superseded,
            "expired" => AckResult::Expired,
            "refused" => AckResult::Refused,
            "failed" => AckResult::Failed,
            "ignored" => AckResult::Ignored,
            _ => return None,
        })
    }
}

/// A machine→guardian gift wrap offered to the ward's local outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalOffer {
    /// The inner rumor kind (REQUEST, STATUS, AUDIT, …), which sets the
    /// outbox retention policy.
    pub inner_kind: u16,
    /// Replacement key: a newer offer with the same key replaces the older
    /// one (used for STATUS, one per subject). `None` keeps every offer.
    pub key: Option<String>,
    /// The exact wrap also published to relays (same event id).
    pub wrap: NostrEvent,
}

/// Receives every machine→guardian wrap before relay IO starts.
///
/// Implementations must return promptly: they may record the offer locally
/// but never wait on the network.
pub trait LocalSink: Send + Sync {
    fn offer(&self, offer: LocalOffer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_result_round_trips_its_wire_spelling() {
        for r in [
            AckResult::Applied,
            AckResult::Unchanged,
            AckResult::Superseded,
            AckResult::Expired,
            AckResult::Refused,
            AckResult::Failed,
            AckResult::Ignored,
        ] {
            assert_eq!(AckResult::parse(r.as_str()), Some(r));
            assert_eq!(
                serde_json::to_string(&r).unwrap(),
                format!("\"{}\"", r.as_str())
            );
        }
        assert_eq!(AckResult::parse("Applied"), None);
    }
}
