//! `bunker://` pairing pin. Reuses Charter's `kind:'charter'` trusted-app
//! pairing semantics: the connect URI must advertise `kind=charter`, carry at
//! least one `wss://` relay, and a valid guardian pubkey. Re-pairing
//! (`repair`) is explicit + audited; the *authorization* boundary that makes it
//! admin-only lives in Phase 9 packaging.

use serde::{Deserialize, Serialize};

use charter_primitives::{uri::percent_decode, PubKey};
use charter_sys::relay::RelayUrl;

/// A pinned guardian pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub guardian_pubkey: PubKey,
    pub relays: Vec<RelayUrl>,
    pub machine: PubKey,
    pub subject_pubkey: PubKey,
    pub audit_transparency: bool,
    pub paired_at: u64,
}

/// Why a `bunker://` URI was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingError {
    NotBunkerUri,
    BadGuardianPubkey,
    MissingRelays,
    NonCharterKind,
}

/// Strip the https App Link wrapper if present, and validate the underlying
/// `bunker://<guardian-pubkey>?relay=wss://..&kind=charter` URI's grammar —
/// a valid guardian pubkey, at least one `wss://` relay, `kind=charter` —
/// without needing a machine, subject or timestamp to pin against. Returns
/// the bare, normalised `bunker://…` string: what `charter-pair --link`
/// (and every other command-line consumer) expects on its argv.
///
/// The QR-scannable form is the https App Link with the bunker URI in its
/// `#fragment` (`https://charter.mysignet.app/pair#bunker://…`). A scan that
/// lands in a browser instead of the app leaves the ward (or the parent,
/// pasting into the console) holding that whole URL, so this accepts it too
/// and pins from the fragment. Trust is unchanged: the link was always
/// attacker-supplyable, and the SAS check on the ward's screen is what
/// defends it, not the scheme it arrived under.
///
/// This is [`pin_from_connect`]'s parse+validate step, factored out so a
/// caller that only has a pasted link — no machine/subject/now yet, e.g. a
/// console or CLI validating what the parent just pasted before shelling out
/// to the privileged pairing helper — can run the *identical* check.
/// `pin_from_connect` calls this too, so the surfaces that accept a pairing
/// link (charterd, `charter-console`, `charter pair`) cannot drift on what
/// counts as a valid one again — see B7,
/// `internal/reviews/2026-09-21/04-linux-lock-tray-cli-packaging.md`.
pub fn validate_and_normalize(link: &str) -> Result<String, PairingError> {
    let bunker_uri = match link.strip_prefix("https://") {
        Some(_) => link
            .split_once('#')
            .map(|(_, frag)| frag)
            .ok_or(PairingError::NotBunkerUri)?,
        None => link,
    };
    let rest = bunker_uri
        .strip_prefix("bunker://")
        .ok_or(PairingError::NotBunkerUri)?;
    let (pubkey_hex, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    PubKey::from_hex(pubkey_hex).map_err(|_| PairingError::BadGuardianPubkey)?;

    let mut has_relay = false;
    let mut kind_charter = false;
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        // Values are percent-decoded BEFORE interpretation: browser guardians
        // (`encodeURIComponent`) emit `relay=wss%3A%2F%2F…`, which the literal
        // prefix check below used to reject wholesale — a latent bug that made
        // every PWA-minted pairing link unpinnable (§5.1). Decoding repairs
        // links already in the field; plain values pass through unchanged.
        let v = percent_decode(v);
        match k {
            "relay" => {
                if v.starts_with("wss://") {
                    has_relay = true;
                } else {
                    // A non-wss relay is rejected as a missing valid relay.
                    return Err(PairingError::MissingRelays);
                }
            }
            "kind" => kind_charter = v == "charter",
            _ => {}
        }
    }

    if !has_relay {
        return Err(PairingError::MissingRelays);
    }
    if !kind_charter {
        return Err(PairingError::NonCharterKind);
    }

    Ok(bunker_uri.to_string())
}

/// Parse + validate a `bunker://<guardian-pubkey>?relay=wss://..&kind=charter`
/// URI (bare, or wrapped in the https App Link form — see
/// [`validate_and_normalize`]), producing a pairing pinned to the given
/// machine + subject.
pub fn pin_from_connect(
    bunker_uri: &str,
    machine: PubKey,
    subject_pubkey: PubKey,
    now: u64,
) -> Result<Pairing, PairingError> {
    let normalized = validate_and_normalize(bunker_uri)?;
    // `validate_and_normalize` has already confirmed this parses — the
    // `expect`s below re-derive fields it already checked, not new fallible
    // parses.
    let rest = normalized
        .strip_prefix("bunker://")
        .expect("validate_and_normalize returns a bare bunker:// URI");
    let (pubkey_hex, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let guardian_pubkey =
        PubKey::from_hex(pubkey_hex).expect("validate_and_normalize already checked the pubkey");

    let mut relays = Vec::new();
    let mut audit_transparency = false;
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v);
        match k {
            "relay" => relays.push(v),
            "audit" => audit_transparency = v == "1" || v == "true",
            _ => {}
        }
    }

    Ok(Pairing {
        guardian_pubkey,
        relays,
        machine,
        subject_pubkey,
        audit_transparency,
        paired_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> PubKey {
        PubKey::from_bytes([0xAA; 32])
    }
    fn subject() -> PubKey {
        PubKey::from_bytes([0xBB; 32])
    }

    fn good_uri() -> String {
        format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            "11".repeat(32)
        )
    }

    #[test]
    fn pins_valid_charter_bunker() {
        let p = pin_from_connect(&good_uri(), machine(), subject(), 100).unwrap();
        assert_eq!(p.relays.len(), 1);
        assert_eq!(p.paired_at, 100);
    }

    /// The QR encodes the https App Link form — `https://charter.mysignet.app/pair#bunker://…`.
    /// When the scan lands in a browser instead of the app, the whole URL is what
    /// the ward can paste (or be handed over adb), so the parser accepts it too.
    #[test]
    fn pins_from_https_app_link_fragment() {
        let uri = format!(
            "https://charter.mysignet.app/pair#{}&token=t0k3n",
            good_uri()
        );
        let p = pin_from_connect(&uri, machine(), subject(), 7).unwrap();
        assert_eq!(
            p.guardian_pubkey,
            PubKey::from_hex(&"11".repeat(32)).unwrap()
        );
        assert_eq!(p.relays.len(), 1);
        assert_eq!(p.paired_at, 7);
    }

    #[test]
    fn rejects_https_link_without_bunker_fragment() {
        assert_eq!(
            pin_from_connect("https://charter.mysignet.app/pair", machine(), subject(), 0),
            Err(PairingError::NotBunkerUri)
        );
        assert_eq!(
            pin_from_connect(
                "https://charter.mysignet.app/pair#nostrconnect://x",
                machine(),
                subject(),
                0
            ),
            Err(PairingError::NotBunkerUri)
        );
    }

    #[test]
    fn rejects_non_bunker_uri() {
        assert_eq!(
            pin_from_connect("nostrconnect://x", machine(), subject(), 0),
            Err(PairingError::NotBunkerUri)
        );
    }

    #[test]
    fn rejects_bad_guardian_pubkey() {
        let uri = "bunker://zzzz?relay=wss://r&kind=charter";
        assert_eq!(
            pin_from_connect(uri, machine(), subject(), 0),
            Err(PairingError::BadGuardianPubkey)
        );
    }

    #[test]
    fn rejects_missing_relays() {
        let uri = format!("bunker://{}?kind=charter", "11".repeat(32));
        assert_eq!(
            pin_from_connect(&uri, machine(), subject(), 0),
            Err(PairingError::MissingRelays)
        );
    }

    #[test]
    fn rejects_non_wss_relay() {
        let uri = format!(
            "bunker://{}?relay=ws://insecure&kind=charter",
            "11".repeat(32)
        );
        assert_eq!(
            pin_from_connect(&uri, machine(), subject(), 0),
            Err(PairingError::MissingRelays)
        );
    }

    #[test]
    fn rejects_non_charter_kind() {
        let uri = format!("bunker://{}?relay=wss://r", "11".repeat(32));
        assert_eq!(
            pin_from_connect(&uri, machine(), subject(), 0),
            Err(PairingError::NonCharterKind)
        );
    }

    /// The §5.1 round-trip pin: the EXACT URI shape older deployed MyCharter
    /// builds emitted (`encodeURIComponent`-encoded relay) must pin. This is
    /// the repair path for pairing links already in the field.
    #[test]
    fn pins_percent_encoded_pwa_uri() {
        let uri = format!(
            "bunker://{}?relay=wss%3A%2F%2Frelay.trotters.cc&kind=charter",
            "11".repeat(32)
        );
        let p = pin_from_connect(&uri, machine(), subject(), 100).unwrap();
        assert_eq!(p.relays, vec!["wss://relay.trotters.cc".to_string()]);
    }

    /// Decoding must not open a hole: an encoded NON-wss relay still rejects.
    #[test]
    fn rejects_percent_encoded_non_wss_relay() {
        let uri = format!(
            "bunker://{}?relay=ws%3A%2F%2Finsecure&kind=charter",
            "11".repeat(32)
        );
        assert_eq!(
            pin_from_connect(&uri, machine(), subject(), 0),
            Err(PairingError::MissingRelays)
        );
    }

    // ---- validate_and_normalize (B7: the console/CLI-facing entry point,
    // no machine/subject/now needed) ----------------------------------------

    #[test]
    fn validate_and_normalize_passes_a_bare_bunker_uri_through_unchanged() {
        let uri = good_uri();
        assert_eq!(validate_and_normalize(&uri).unwrap(), uri);
    }

    #[test]
    fn validate_and_normalize_strips_the_https_app_link_wrapper() {
        let uri = format!("https://charter.mysignet.app/pair#{}", good_uri());
        assert_eq!(validate_and_normalize(&uri).unwrap(), good_uri());
    }

    #[test]
    fn validate_and_normalize_rejects_garbage() {
        for garbage in [
            "not-a-link-at-all",
            "nostrconnect://x",
            "https://charter.mysignet.app/pair",
            "https://charter.mysignet.app/pair#nostrconnect://x",
        ] {
            assert_eq!(
                validate_and_normalize(garbage),
                Err(PairingError::NotBunkerUri),
                "garbage input: {garbage}"
            );
        }
        // Well-formed scheme, but fails validation further in — still
        // rejected, just with a more specific reason.
        let bad_pubkey = "bunker://zzzz?relay=wss://r&kind=charter";
        assert_eq!(
            validate_and_normalize(bad_pubkey),
            Err(PairingError::BadGuardianPubkey)
        );
    }

    /// `pin_from_connect` and `validate_and_normalize` must agree on every
    /// input — the whole point of factoring one through the other. Fuzz a
    /// handful of shapes rather than trust that by inspection alone.
    #[test]
    fn pin_from_connect_and_validate_and_normalize_agree() {
        let cases = [
            good_uri(),
            format!("https://charter.mysignet.app/pair#{}", good_uri()),
            "bunker://zzzz?relay=wss://r&kind=charter".to_string(),
            format!("bunker://{}?kind=charter", "11".repeat(32)),
            format!(
                "bunker://{}?relay=ws://insecure&kind=charter",
                "11".repeat(32)
            ),
            format!("bunker://{}?relay=wss://r", "11".repeat(32)),
            "nostrconnect://x".to_string(),
        ];
        for uri in cases {
            let norm = validate_and_normalize(&uri);
            let pin = pin_from_connect(&uri, machine(), subject(), 0);
            assert_eq!(
                norm.is_ok(),
                pin.is_ok(),
                "disagreement on {uri}: normalize={norm:?} pin={pin:?}"
            );
            if let (Err(a), Err(b)) = (&norm, &pin) {
                assert_eq!(a, b, "different rejection reason for {uri}");
            }
        }
    }
}
