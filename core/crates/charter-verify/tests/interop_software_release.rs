//! Interop: a REAL nostr-tools-signed kind-30063 software-release event
//! verifies end-to-end in Rust with identical parsed fields. The fixture is
//! produced by `scripts/gen-nostr-vectors.mjs` with a throwaway TEST key.

#![cfg(feature = "mock")]

use charter_primitives::{NostrEvent, PubKey};
use charter_verify::{verify_software_release, SoftwareReleaseError};

#[derive(serde::Deserialize)]
struct Expected {
    channel: String,
    #[serde(rename = "versionName")]
    version_name: String,
    #[serde(rename = "versionCode")]
    version_code: u64,
    sha256: String,
    #[serde(rename = "sizeBytes")]
    size_bytes: u64,
    #[serde(rename = "certSha256")]
    cert_sha256: String,
    urls: Vec<String>,
}

#[derive(serde::Deserialize)]
struct Vector {
    event: NostrEvent,
    release_pubkey: PubKey,
    expected: Expected,
}

fn load() -> Vector {
    charter_testkit::golden::load_json("nostr/software_release.json")
}

#[test]
fn a_nostr_tools_signed_release_verifies_with_identical_fields() {
    let v = load();
    let r = verify_software_release(&v.event, &v.release_pubkey, &v.expected.channel)
        .expect("golden release event must verify");
    assert_eq!(r.channel, v.expected.channel);
    assert_eq!(r.version_name, v.expected.version_name);
    assert_eq!(r.version_code, v.expected.version_code);
    assert_eq!(r.sha256.to_hex(), v.expected.sha256);
    assert_eq!(r.size_bytes, v.expected.size_bytes);
    assert_eq!(r.cert_sha256.unwrap().to_hex(), v.expected.cert_sha256);
    assert_eq!(r.urls, v.expected.urls);
}

#[test]
fn a_tampered_golden_release_fails_id_integrity() {
    let mut v = load();
    for t in v.event.tags.iter_mut() {
        if t[0] == "version_code" {
            t[1] = "9999".into();
        }
    }
    assert_eq!(
        verify_software_release(&v.event, &v.release_pubkey, &v.expected.channel),
        Err(SoftwareReleaseError::BadId)
    );
}

#[test]
fn the_golden_release_is_rejected_under_a_different_pin() {
    let v = load();
    let other = PubKey::from_bytes([0x42; 32]);
    assert_eq!(
        verify_software_release(&v.event, &other, &v.expected.channel),
        Err(SoftwareReleaseError::WrongAuthor)
    );
}
