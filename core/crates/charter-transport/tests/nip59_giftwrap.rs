//! Cross-impl NIP-59 parity: Rust unwraps a gift-wrap produced by nostr-tools
//! and recovers the exact rumor, with the sealed-author binding enforced.

use charter_primitives::{NostrEvent, PubKey, Sha256Hex};
use charter_transport::nip59::{unwrap, Nip59Error, MAX_JITTER_SECS};

#[derive(serde::Deserialize)]
struct Fixture {
    recipient_secret: Sha256Hex,
    #[allow(dead_code)]
    recipient_pubkey: PubKey,
    sender_pubkey: PubKey,
    wrap: NostrEvent,
    expected_rumor: Expected,
}

#[derive(serde::Deserialize)]
struct Expected {
    pubkey: PubKey,
    kind: u16,
    content: String,
}

fn fixture() -> Fixture {
    charter_testkit::golden::load_json("nostr/nip59_giftwrap.json")
}

#[test]
fn rust_unwraps_nostr_tools_giftwrap() {
    let f = fixture();
    let rumor = unwrap(
        &f.wrap,
        f.recipient_secret.as_bytes(),
        f.wrap.created_at, // jitter 0 — stable across time
        MAX_JITTER_SECS,
    )
    .unwrap();
    assert_eq!(rumor.pubkey, f.sender_pubkey);
    assert_eq!(rumor.pubkey, f.expected_rumor.pubkey);
    assert_eq!(rumor.kind, f.expected_rumor.kind);
    assert_eq!(rumor.content, f.expected_rumor.content);
}

#[test]
fn wrong_recipient_fails_to_unwrap() {
    let f = fixture();
    let attacker = [0x99u8; 32];
    assert!(unwrap(&f.wrap, &attacker, f.wrap.created_at, MAX_JITTER_SECS).is_err());
}

#[test]
fn tampered_wrap_signature_rejected() {
    let mut f = fixture();
    let mut sig = *f.wrap.sig.as_bytes();
    sig[0] ^= 0x01;
    f.wrap.sig = charter_primitives::Sig::from_bytes(sig);
    assert_eq!(
        unwrap(
            &f.wrap,
            f.recipient_secret.as_bytes(),
            f.wrap.created_at,
            MAX_JITTER_SECS
        ),
        Err(Nip59Error::BadWrapSignature)
    );
}
