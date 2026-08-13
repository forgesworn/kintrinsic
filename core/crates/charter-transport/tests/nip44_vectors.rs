//! NIP-44 v2 KAT parity against real nostr-tools output: every valid vector
//! reproduces exactly (same ciphertext) and decrypts; every invalid case errors
//! with no plaintext leak.

use charter_primitives::{PubKey, Sha256Hex};
use charter_transport::nip44::{conversation_key, decrypt, encrypt};

#[derive(serde::Deserialize)]
struct Vectors {
    sender_secret: Sha256Hex,
    recipient_pubkey: PubKey,
    conversation_key: Sha256Hex,
    valid: Vec<Valid>,
    invalid: Vec<Invalid>,
}

#[derive(serde::Deserialize)]
struct Valid {
    nonce: Sha256Hex,
    plaintext: String,
    payload: String,
}

#[derive(serde::Deserialize)]
struct Invalid {
    reason: String,
    payload: String,
}

fn vectors() -> Vectors {
    charter_testkit::golden::load_json("nostr/nip44_vectors.json")
}

#[test]
fn conversation_key_matches_nostr_tools() {
    let v = vectors();
    let ck = conversation_key(v.sender_secret.as_bytes(), v.recipient_pubkey.as_bytes()).unwrap();
    assert_eq!(&ck, v.conversation_key.as_bytes());
}

#[test]
fn every_valid_vector_reproduces_and_decrypts() {
    let v = vectors();
    let ck = *v.conversation_key.as_bytes();
    for case in &v.valid {
        let payload = encrypt(&case.plaintext, &ck, case.nonce.as_bytes()).unwrap();
        assert_eq!(
            payload, case.payload,
            "ciphertext must byte-match nostr-tools"
        );
        assert_eq!(decrypt(&case.payload, &ck).unwrap(), case.plaintext);
    }
}

#[test]
fn every_invalid_vector_errors_without_leaking_plaintext() {
    let v = vectors();
    let ck = *v.conversation_key.as_bytes();
    for case in &v.invalid {
        let result = decrypt(&case.payload, &ck);
        assert!(
            result.is_err(),
            "invalid case {} must not decrypt",
            case.reason
        );
    }
}
