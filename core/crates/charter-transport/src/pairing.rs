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

/// Parse + validate a `bunker://<guardian-pubkey>?relay=wss://..&kind=charter`
/// URI, producing a pairing pinned to the given machine + subject.
pub fn pin_from_connect(
    bunker_uri: &str,
    machine: PubKey,
    subject_pubkey: PubKey,
    now: u64,
) -> Result<Pairing, PairingError> {
    let rest = bunker_uri
        .strip_prefix("bunker://")
        .ok_or(PairingError::NotBunkerUri)?;
    let (pubkey_hex, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let guardian_pubkey =
        PubKey::from_hex(pubkey_hex).map_err(|_| PairingError::BadGuardianPubkey)?;

    let mut relays = Vec::new();
    let mut kind_charter = false;
    let mut audit_transparency = false;
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
                    relays.push(v);
                } else {
                    // A non-wss relay is rejected as a missing valid relay.
                    return Err(PairingError::MissingRelays);
                }
            }
            "kind" => kind_charter = v == "charter",
            "audit" => audit_transparency = v == "1" || v == "true",
            _ => {}
        }
    }

    if relays.is_empty() {
        return Err(PairingError::MissingRelays);
    }
    if !kind_charter {
        return Err(PairingError::NonCharterKind);
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
}
