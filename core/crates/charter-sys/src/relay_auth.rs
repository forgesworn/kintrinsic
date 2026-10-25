//! NIP-42 proofs authenticate a connection with its stable device/guardian
//! identity. Gift-wrap outer authors remain ephemeral for privacy.

use crate::{signer::MachineSigner, SysError, SysResult};
use charter_primitives::{EventId, NostrEvent, Sig};

pub const AUTH_KIND: u16 = 22242;

pub fn auth_event(
    signer: &dyn MachineSigner,
    relay: &str,
    challenge: &str,
    now: u64,
) -> SysResult<NostrEvent> {
    if challenge.is_empty() || challenge.len() > 4096 {
        return Err(SysError::Crypto(
            "invalid relay authentication challenge".into(),
        ));
    }
    let mut event = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey: signer.pubkey(),
        created_at: now,
        kind: AUTH_KIND,
        tags: vec![
            vec!["relay".into(), relay.into()],
            vec!["challenge".into(), challenge.into()],
        ],
        content: String::new(),
        sig: Sig::from_bytes([0; 64]),
    };
    let hash = charter_crypto::sha256(charter_proto::canonical_event_string(&event).as_bytes());
    event.id = EventId::from_bytes(hash);
    event.sig = signer.sign(&hash)?;
    Ok(event)
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::signer::{MachineSigner, SeedSigner};

    #[test]
    fn proof_is_signed_by_the_stable_identity_and_bound_to_the_connection() {
        let signer = SeedSigner::from_seed(7);
        let proof = auth_event(&signer, "wss://private.example", "challenge-123", 100).unwrap();
        assert_eq!(proof.pubkey, signer.pubkey());
        assert_eq!(proof.kind, 22242);
        assert_eq!(proof.created_at, 100);
        assert!(proof.content.is_empty());
        assert!(proof.has_tag("relay", "wss://private.example"));
        assert!(proof.has_tag("challenge", "challenge-123"));
        let hash = charter_crypto::sha256(charter_proto::canonical_event_string(&proof).as_bytes());
        assert_eq!(proof.id.as_bytes(), &hash);
        assert!(charter_crypto::schnorr_verify(
            proof.pubkey.as_bytes(),
            &hash,
            proof.sig.as_bytes()
        ));
        assert!(auth_event(&signer, "wss://private.example", "", 100).is_err());
    }

    #[test]
    fn auth_proof_matches_the_nostr_tools_event_id() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../charter-testkit/vectors/nostr/relay_auth.json"
        ))
        .unwrap();
        let signer = SeedSigner::from_seed(0x11);
        let proof = auth_event(
            &signer,
            fixture["relay"].as_str().unwrap(),
            fixture["challenge"].as_str().unwrap(),
            fixture["event"]["created_at"].as_u64().unwrap(),
        )
        .unwrap();
        assert_eq!(proof.id.to_hex(), fixture["event"]["id"].as_str().unwrap());
        assert_eq!(
            proof.pubkey.to_hex(),
            fixture["event"]["pubkey"].as_str().unwrap()
        );
    }
}
