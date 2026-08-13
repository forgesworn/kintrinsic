//! `verify_software_release` — authenticate a kind-30063 SOFTWARE RELEASE
//! announcement against the compiled-in **release key** (a separate trust
//! anchor from the pinned guardian: it signs "the latest artifact for channel
//! X is these bytes", never device policy).
//!
//! Unlike `verify_release` (the unpair event) there is deliberately **no
//! freshness window**: a release event stays valid for as long as the fleet
//! might need it. Downgrade safety is the caller's version comparison — a
//! replayed old event offers an equal-or-lower `version_code` and is a no-op.

use charter_primitives::{kinds, NostrEvent, PubKey, Sha256Hex};

use crate::event::{id_is_consistent, signature_is_valid};

/// Why a software-release event was rejected. Every `Err` means "ignore it".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoftwareReleaseError {
    /// Not kind 30063.
    WrongKind,
    /// The `d` tag is missing or names a different channel.
    WrongChannel,
    /// Event-id integrity failed (content does not hash to `id`).
    BadId,
    /// Schnorr signature failed.
    BadSig,
    /// Signed by a key other than the pinned release key.
    WrongAuthor,
    /// Structurally signed fine but a required field is missing/invalid.
    BadShape(&'static str),
}

/// The parsed, authenticated announcement for one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftwareRelease {
    /// The release channel (the `d` tag).
    pub channel: String,
    /// Human version, e.g. `0.6.9` (the `version` tag).
    pub version_name: String,
    /// Monotonic comparable code (the `version_code` tag).
    pub version_code: u64,
    /// sha256 of the artifact bytes (the `x` tag).
    pub sha256: Sha256Hex,
    /// Artifact size in bytes (the `size` tag).
    pub size_bytes: u64,
    /// Every `url` tag, in event order; each is https. Content-addressed
    /// mirrors — any one that serves bytes matching `sha256` is as good as
    /// another.
    pub urls: Vec<String>,
    /// APK channels: the Android signing-cert sha256 (the `cert` tag).
    pub cert_sha256: Option<Sha256Hex>,
    /// Event timestamp (informational; NOT a validity bound).
    pub created_at: u64,
}

/// Authenticate a SOFTWARE RELEASE event for `channel`, signed by the pinned
/// release key. Order: kind → channel → id integrity → signature → pinned
/// author → shape.
pub fn verify_software_release(
    event: &NostrEvent,
    pinned_release_key: &PubKey,
    channel: &str,
) -> Result<SoftwareRelease, SoftwareReleaseError> {
    if event.kind != kinds::SOFTWARE_RELEASE {
        return Err(SoftwareReleaseError::WrongKind);
    }
    match event.tag_value("d") {
        Some(d) if d == channel => {}
        _ => return Err(SoftwareReleaseError::WrongChannel),
    }
    if !id_is_consistent(event) {
        return Err(SoftwareReleaseError::BadId);
    }
    if !signature_is_valid(event) {
        return Err(SoftwareReleaseError::BadSig);
    }
    if &event.pubkey != pinned_release_key {
        return Err(SoftwareReleaseError::WrongAuthor);
    }

    let version_name = event
        .tag_value("version")
        .filter(|v| !v.is_empty())
        .ok_or(SoftwareReleaseError::BadShape("version"))?
        .to_string();
    let version_code: u64 = event
        .tag_value("version_code")
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .ok_or(SoftwareReleaseError::BadShape("version_code"))?;
    let sha256 = event
        .tag_value("x")
        .and_then(|v| Sha256Hex::from_hex(v).ok())
        .ok_or(SoftwareReleaseError::BadShape("x"))?;
    let size_bytes: u64 = event
        .tag_value("size")
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .ok_or(SoftwareReleaseError::BadShape("size"))?;
    let urls: Vec<String> = event
        .tags
        .iter()
        .filter(|t| t.len() >= 2 && t[0] == "url")
        .map(|t| t[1].clone())
        .collect();
    if urls.is_empty() {
        return Err(SoftwareReleaseError::BadShape("url"));
    }
    if urls.iter().any(|u| !u.starts_with("https://")) {
        return Err(SoftwareReleaseError::BadShape("url-not-https"));
    }
    let cert_sha256 = match event.tag_value("cert") {
        None => None,
        Some(c) => {
            Some(Sha256Hex::from_hex(c).map_err(|_| SoftwareReleaseError::BadShape("cert"))?)
        }
    };

    Ok(SoftwareRelease {
        channel: channel.to_string(),
        version_name,
        version_code,
        sha256,
        size_bytes,
        urls,
        cert_sha256,
        created_at: event.created_at,
    })
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::test_support::{sign_event, TestGuardian};
    use charter_sys::signer::SeedSigner;

    const NOW: u64 = 1_754_900_000;
    const SHA: &str = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
    const CERT: &str = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";

    fn tags(channel: &str) -> Vec<Vec<String>> {
        vec![
            vec!["d".into(), channel.into()],
            vec!["version".into(), "0.6.9".into()],
            vec!["version_code".into(), "40".into()],
            vec!["x".into(), SHA.into()],
            vec!["size".into(), "27693181".into()],
            vec!["cert".into(), CERT.into()],
            vec!["url".into(), format!("https://blossom.example/{SHA}")],
            vec!["url".into(), format!("https://mirror.example/{SHA}")],
        ]
    }

    fn release_event(signer: &SeedSigner, tags: Vec<Vec<String>>) -> NostrEvent {
        sign_event(
            signer,
            kinds::SOFTWARE_RELEASE,
            NOW,
            tags,
            "release notes".into(),
        )
    }

    fn without(tags: Vec<Vec<String>>, name: &str) -> Vec<Vec<String>> {
        tags.into_iter().filter(|t| t[0] != name).collect()
    }

    #[test]
    fn a_release_key_signed_announcement_verifies_and_maps_every_field() {
        let key = TestGuardian::new();
        let ev = release_event(&key.signer, tags("charter-apk"));
        let r = verify_software_release(&ev, &key.pubkey(), "charter-apk").unwrap();
        assert_eq!(r.channel, "charter-apk");
        assert_eq!(r.version_name, "0.6.9");
        assert_eq!(r.version_code, 40);
        assert_eq!(r.sha256.to_hex(), SHA);
        assert_eq!(r.size_bytes, 27_693_181);
        assert_eq!(
            r.urls,
            vec![
                format!("https://blossom.example/{SHA}"),
                format!("https://mirror.example/{SHA}"),
            ]
        );
        assert_eq!(r.cert_sha256.unwrap().to_hex(), CERT);
        assert_eq!(r.created_at, NOW);
    }

    #[test]
    fn a_deb_release_without_cert_is_fine() {
        let key = TestGuardian::new();
        let ev = release_event(&key.signer, without(tags("charter-deb"), "cert"));
        let r = verify_software_release(&ev, &key.pubkey(), "charter-deb").unwrap();
        assert_eq!(r.cert_sha256, None);
    }

    #[test]
    fn another_signer_is_rejected_even_with_a_valid_signature() {
        let pinned = TestGuardian::new();
        let attacker = SeedSigner::from_seed(0x99);
        let ev = release_event(&attacker, tags("charter-apk"));
        assert_eq!(
            verify_software_release(&ev, &pinned.pubkey(), "charter-apk"),
            Err(SoftwareReleaseError::WrongAuthor)
        );
    }

    #[test]
    fn tampering_with_a_tag_after_signing_fails_id_integrity() {
        let key = TestGuardian::new();
        let mut ev = release_event(&key.signer, tags("charter-apk"));
        for t in ev.tags.iter_mut() {
            if t[0] == "version_code" {
                t[1] = "9999".into();
            }
        }
        assert_eq!(
            verify_software_release(&ev, &key.pubkey(), "charter-apk"),
            Err(SoftwareReleaseError::BadId)
        );
    }

    #[test]
    fn a_forged_signature_fails() {
        let key = TestGuardian::new();
        let mut ev = release_event(&key.signer, tags("charter-apk"));
        // Re-stamp a consistent id for altered content, but keep the old sig.
        for t in ev.tags.iter_mut() {
            if t[0] == "version_code" {
                t[1] = "9999".into();
            }
        }
        ev.id = crate::event::event_id(&ev);
        assert_eq!(
            verify_software_release(&ev, &key.pubkey(), "charter-apk"),
            Err(SoftwareReleaseError::BadSig)
        );
    }

    #[test]
    fn the_wrong_channel_is_rejected() {
        let key = TestGuardian::new();
        let ev = release_event(&key.signer, tags("charter-apk"));
        assert_eq!(
            verify_software_release(&ev, &key.pubkey(), "mycharter-apk"),
            Err(SoftwareReleaseError::WrongChannel)
        );
    }

    #[test]
    fn the_wrong_kind_is_rejected() {
        let key = TestGuardian::new();
        let ev = sign_event(
            &key.signer,
            kinds::CHARTER_DEVICE_RELEASE,
            NOW,
            tags("charter-apk"),
            String::new(),
        );
        assert_eq!(
            verify_software_release(&ev, &key.pubkey(), "charter-apk"),
            Err(SoftwareReleaseError::WrongKind)
        );
    }

    #[test]
    fn missing_or_invalid_required_fields_are_rejected() {
        let key = TestGuardian::new();
        let pk = key.pubkey();
        let cases: Vec<(Vec<Vec<String>>, &str)> = vec![
            (without(tags("charter-apk"), "x"), "missing x"),
            (without(tags("charter-apk"), "version"), "missing version"),
            (
                without(tags("charter-apk"), "version_code"),
                "missing version_code",
            ),
            (without(tags("charter-apk"), "size"), "missing size"),
            (without(tags("charter-apk"), "url"), "missing urls"),
        ];
        for (tags, what) in cases {
            let ev = release_event(&key.signer, tags);
            assert!(
                matches!(
                    verify_software_release(&ev, &pk, "charter-apk"),
                    Err(SoftwareReleaseError::BadShape(_))
                ),
                "{what} must be BadShape"
            );
        }
    }

    #[test]
    fn zero_version_code_and_uppercase_sha_are_rejected() {
        let key = TestGuardian::new();
        let pk = key.pubkey();

        let mut zero = tags("charter-apk");
        for t in zero.iter_mut() {
            if t[0] == "version_code" {
                t[1] = "0".into();
            }
        }
        let ev = release_event(&key.signer, zero);
        assert_eq!(
            verify_software_release(&ev, &pk, "charter-apk"),
            Err(SoftwareReleaseError::BadShape("version_code"))
        );

        let mut upper = tags("charter-apk");
        for t in upper.iter_mut() {
            if t[0] == "x" {
                t[1] = SHA.to_uppercase();
            }
        }
        let ev = release_event(&key.signer, upper);
        assert_eq!(
            verify_software_release(&ev, &pk, "charter-apk"),
            Err(SoftwareReleaseError::BadShape("x"))
        );
    }

    #[test]
    fn a_plain_http_mirror_is_rejected() {
        let key = TestGuardian::new();
        let mut t = tags("charter-apk");
        t.push(vec!["url".into(), format!("http://evil.example/{SHA}")]);
        let ev = release_event(&key.signer, t);
        assert_eq!(
            verify_software_release(&ev, &key.pubkey(), "charter-apk"),
            Err(SoftwareReleaseError::BadShape("url-not-https"))
        );
    }
}
