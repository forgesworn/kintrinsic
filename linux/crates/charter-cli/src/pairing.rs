//! `bunker://` URI validation for `charter pair`. Kept to behavioral PARITY with
//! `charter_transport::pin_from_connect` (the daemon-side pin): a `bunker://`
//! URI carrying a valid guardian pubkey, at least one `wss://` relay, and
//! `kind=charter`. A weaker CLI check (the old version only looked at relay
//! schemes) accepted URIs the daemon then rejected — a confusing late failure.
//! M19: single-source the strict pubkey decoder via `charter-primitives`.
//!
//! NOTE (cross-repo / RANK4): whether `kind=charter` belongs in the `bunker://`
//! URI at all (vs the app-initiated `nostrconnect://` connect metadata) is a
//! contract.md decision pending with the Signet team; this validator stays in
//! lockstep with `pin_from_connect` so both move together when that is settled.

use charter_primitives::{uri::percent_decode, PubKey};

/// Why a pairing URI was rejected at the CLI boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UriError {
    /// Not a `bunker://` URI.
    NotBunker,
    /// The guardian pubkey (before `?`) is not valid 64-hex.
    BadGuardianPubkey,
    /// A `relay=` was present but not `wss://`.
    HttpRelay,
    /// No `wss://` relay present.
    MissingRelays,
    /// `kind=charter` was not present.
    NonCharterKind,
}

/// Validate `bunker://<guardian-pubkey>?relay=wss://..&kind=charter`.
pub fn validate_bunker_uri(uri: &str) -> Result<(), UriError> {
    let rest = uri.strip_prefix("bunker://").ok_or(UriError::NotBunker)?;
    let (pubkey_hex, query) = rest.split_once('?').unwrap_or((rest, ""));
    PubKey::from_hex(pubkey_hex).map_err(|_| UriError::BadGuardianPubkey)?;
    let mut has_relay = false;
    let mut kind_charter = false;
    for pair in query.split('&') {
        // Percent-decode values before interpreting, in lockstep with
        // `pin_from_connect` (§5.1): browser guardians emit
        // `relay=wss%3A%2F%2F…`, which the literal prefix check rejected.
        match pair.split_once('=') {
            Some(("relay", v)) => {
                if !percent_decode(v).starts_with("wss://") {
                    return Err(UriError::HttpRelay);
                }
                has_relay = true;
            }
            Some(("kind", v)) => kind_charter = percent_decode(v) == "charter",
            _ => {}
        }
    }
    if !has_relay {
        return Err(UriError::MissingRelays);
    }
    if !kind_charter {
        return Err(UriError::NonCharterKind);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 64-hex guardian pubkey for URIs under test.
    const PK: &str = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";

    #[test]
    fn accepts_wss_bunker() {
        let uri = format!("bunker://{PK}?relay=wss://r.example&kind=charter");
        assert!(validate_bunker_uri(&uri).is_ok());
    }

    #[test]
    fn rejects_non_bunker() {
        assert_eq!(
            validate_bunker_uri("nostrconnect://x"),
            Err(UriError::NotBunker)
        );
    }

    #[test]
    fn rejects_http_relay() {
        let uri = format!("bunker://{PK}?relay=ws://insecure&kind=charter");
        assert_eq!(validate_bunker_uri(&uri), Err(UriError::HttpRelay));
    }

    #[test]
    fn rejects_bad_guardian_pubkey() {
        assert_eq!(
            validate_bunker_uri("bunker://abc?relay=wss://r&kind=charter"),
            Err(UriError::BadGuardianPubkey)
        );
    }

    #[test]
    fn rejects_uri_without_relay() {
        let uri = format!("bunker://{PK}?kind=charter");
        assert_eq!(validate_bunker_uri(&uri), Err(UriError::MissingRelays));
    }

    #[test]
    fn rejects_uri_without_kind_charter() {
        let uri = format!("bunker://{PK}?relay=wss://r.example");
        assert_eq!(validate_bunker_uri(&uri), Err(UriError::NonCharterKind));
    }

    /// §5.1 parity with `pin_from_connect`: the percent-encoded relay shape
    /// older Kintrinsic builds emitted must validate.
    #[test]
    fn accepts_percent_encoded_relay() {
        let uri = format!("bunker://{PK}?relay=wss%3A%2F%2Frelay.trotters.cc&kind=charter");
        assert!(validate_bunker_uri(&uri).is_ok());
    }

    #[test]
    fn rejects_percent_encoded_non_wss_relay() {
        let uri = format!("bunker://{PK}?relay=ws%3A%2F%2Finsecure&kind=charter");
        assert_eq!(validate_bunker_uri(&uri), Err(UriError::HttpRelay));
    }
}
