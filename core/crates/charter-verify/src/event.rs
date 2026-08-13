//! NIP-01 event-id computation + integrity/signature checks over the single
//! `charter-crypto` backend.

use charter_primitives::{EventId, NostrEvent};
use charter_proto::canonical_event_string;

/// The NIP-01 event id: sha256 of the canonical serialization.
pub fn event_id(ev: &NostrEvent) -> EventId {
    EventId::from_bytes(charter_crypto::sha256(
        canonical_event_string(ev).as_bytes(),
    ))
}

/// Whether the event's claimed `id` matches its content (integrity).
pub fn id_is_consistent(ev: &NostrEvent) -> bool {
    event_id(ev) == ev.id
}

/// Whether the schnorr signature over the (recomputed) id is valid for the
/// event's author key. Does **not** check authority (who the author is).
pub fn signature_is_valid(ev: &NostrEvent) -> bool {
    let id = event_id(ev);
    charter_crypto::schnorr_verify(ev.pubkey.as_bytes(), id.as_bytes(), ev.sig.as_bytes())
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;

    #[test]
    fn golden_event_id_matches_nostr_tools() {
        #[derive(serde::Deserialize)]
        struct V {
            event: NostrEvent,
        }
        let v: V = charter_testkit::golden::load_json("nostr/nip01_event.json");
        assert!(
            id_is_consistent(&v.event),
            "recomputed id must match nostr-tools id"
        );
    }

    #[test]
    fn golden_grant_signature_verifies() {
        #[derive(serde::Deserialize)]
        struct V {
            event: NostrEvent,
        }
        let v: V = charter_testkit::golden::load_json("nostr/grant_install_allow.json");
        assert!(id_is_consistent(&v.event));
        assert!(
            signature_is_valid(&v.event),
            "nostr-tools BIP-340 sig must verify"
        );
    }
}
