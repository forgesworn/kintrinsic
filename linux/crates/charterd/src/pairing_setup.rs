//! The testable core of graphical guardian pairing: turn the `bunker://…` link
//! the parent pastes from Kintrinsic into the JSON the daemon's pairing store
//! holds, with normie error text. Validation reuses the single source of truth
//! `charter_transport::pin_from_connect` (kept in lockstep with the CLI
//! `charter pair` validator + `spec/contract.md`). Pure — the privileged file
//! write + `systemctl try-restart` live in the `charter-pair` binary.

use charter_primitives::PubKey;
use charter_transport::pairing::{pin_from_connect, PairingError};

/// Validate the `bunker://` link and build the pinned-pairing JSON for the
/// daemon's `pairing.json`. The link carries only PUBLIC data (guardian pubkey +
/// wss relays + kind), so the result is safe to write world-readable.
pub fn build_pairing_json(
    bunker_uri: &str,
    machine: PubKey,
    subject: PubKey,
    now: u64,
) -> Result<String, PairingError> {
    let pairing = pin_from_connect(bunker_uri.trim(), machine, subject, now)?;
    Ok(serde_json::to_string_pretty(&pairing).expect("pairing serializes"))
}

/// Normie-facing text for each rejection reason (shown in the zenity error).
pub fn map_pairing_error(e: &PairingError) -> &'static str {
    match e {
        PairingError::NotBunkerUri => {
            "That's not a pairing link. In Kintrinsic, copy the link that starts with \"bunker://\"."
        }
        PairingError::BadGuardianPubkey => {
            "That pairing link looks corrupted. Copy it again from Kintrinsic."
        }
        PairingError::MissingRelays => {
            "That pairing link is missing a secure relay (wss://). Copy the whole link from Kintrinsic."
        }
        PairingError::NonCharterKind => {
            "That isn't a Kintrinsic pairing link. In Kintrinsic choose \"Pair Kintrinsic\", not a regular app."
        }
    }
}

/// Read this device's machine pubkey from the world-readable `device.pub`
/// (written by the daemon at boot) — the `machine` for the pairing.
pub fn read_device_pub(path: &str) -> Option<PubKey> {
    let contents = std::fs::read_to_string(path).ok()?;
    let hex = crate::device_code::parse_device_pub(&contents).ok()?;
    PubKey::from_hex(&hex).ok()
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
    fn guardian_hex() -> String {
        PubKey::from_bytes([0xCC; 32]).to_hex()
    }

    #[test]
    fn build_pairing_json_accepts_a_valid_charter_bunker() {
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian_hex()
        );
        let json = build_pairing_json(&uri, machine(), subject(), 1234).unwrap();
        let back: charter_transport::pairing::Pairing = serde_json::from_str(&json).unwrap();
        assert_eq!(back.guardian_pubkey.to_hex(), guardian_hex());
        assert_eq!(back.machine, machine());
        assert_eq!(back.subject_pubkey, subject());
        assert_eq!(back.relays, vec!["wss://relay.example".to_string()]);
        assert_eq!(back.paired_at, 1234);
    }

    #[test]
    fn build_pairing_json_rejects_each_way() {
        // not a bunker uri
        assert_eq!(
            build_pairing_json("https://nope", machine(), subject(), 0),
            Err(PairingError::NotBunkerUri)
        );
        // bad guardian pubkey
        assert_eq!(
            build_pairing_json(
                "bunker://nothex?relay=wss://r&kind=charter",
                machine(),
                subject(),
                0
            ),
            Err(PairingError::BadGuardianPubkey)
        );
        // ws:// (not wss) folds into MissingRelays
        assert_eq!(
            build_pairing_json(
                &format!("bunker://{}?relay=ws://r&kind=charter", guardian_hex()),
                machine(),
                subject(),
                0
            ),
            Err(PairingError::MissingRelays)
        );
        // missing kind=charter
        assert_eq!(
            build_pairing_json(
                &format!("bunker://{}?relay=wss://r", guardian_hex()),
                machine(),
                subject(),
                0
            ),
            Err(PairingError::NonCharterKind)
        );
    }

    #[test]
    fn map_pairing_error_is_distinct_nonempty_text_for_each() {
        let msgs = [
            map_pairing_error(&PairingError::NotBunkerUri),
            map_pairing_error(&PairingError::BadGuardianPubkey),
            map_pairing_error(&PairingError::MissingRelays),
            map_pairing_error(&PairingError::NonCharterKind),
        ];
        assert!(msgs.iter().all(|m| !m.is_empty()));
        let unique: std::collections::BTreeSet<_> = msgs.iter().collect();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn read_device_pub_parses_a_hex_file_and_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("charter-pp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let good = dir.join("good.pub");
        let code = crate::device_code::device_pairing_code(&[7u8; 32]).unwrap();
        std::fs::write(&good, format!("{code}\n")).unwrap();
        assert_eq!(
            read_device_pub(good.to_str().unwrap()).map(|p| p.to_hex()),
            Some(code)
        );
        let bad = dir.join("bad.pub");
        std::fs::write(&bad, "not a key").unwrap();
        assert!(read_device_pub(bad.to_str().unwrap()).is_none());
        assert!(read_device_pub(dir.join("absent.pub").to_str().unwrap()).is_none());
    }
}
