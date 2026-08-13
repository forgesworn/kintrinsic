//! The device's **pairing code** — the hex of its machine x-only pubkey. The
//! parent enters this in MyCharter so a guardian's clauses are gift-wrapped to
//! THIS device. MyCharter accepts hex (or npub/nprofile); charterd emits plain
//! hex, so no bech32 dependency is needed on the device side. The pubkey is
//! public — the daemon writes it world-readable + logs it at boot (see
//! `runtime::run`).

use charter_primitives::PubKey;

/// The pairing code for a machine secret: the 64-char hex x-only pubkey, or
/// `None` if the secret isn't a valid signing scalar (an unprovisioned/corrupt
/// key — fail-safe: no code rather than a bogus one).
pub fn device_pairing_code(secret: &[u8; 32]) -> Option<String> {
    charter_crypto::xonly_pubkey(secret)
        .ok()
        .map(|pk| PubKey::from_bytes(pk).to_hex())
}

/// Why the on-screen viewer couldn't show a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCodeError {
    /// The `device.pub` file doesn't exist yet (daemon hasn't booted).
    Missing,
    /// The file is present but empty/whitespace.
    Empty,
    /// The contents aren't a 64-char hex pubkey.
    Malformed,
}

/// Parse the contents of `device.pub` into the canonical 64-hex code the viewer
/// shows (and the parent enters in MyCharter). Trims trailing newline/space; the
/// caller maps a missing file to [`DeviceCodeError::Missing`].
pub fn parse_device_pub(contents: &str) -> Result<String, DeviceCodeError> {
    let c = contents.trim();
    if c.is_empty() {
        return Err(DeviceCodeError::Empty);
    }
    if c.len() == 64 && c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(c.to_lowercase());
    }
    Err(DeviceCodeError::Malformed)
}

/// Group a 64-hex code into 8 space-separated 8-char blocks for readable
/// on-screen display. Stripping the spaces yields the raw code (what the QR
/// encodes and what the parent copies).
pub fn grouped_for_display(code: &str) -> String {
    code.as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_the_64_hex_xonly_pubkey_and_deterministic() {
        let secret = [7u8; 32];
        let code = device_pairing_code(&secret).expect("valid scalar -> code");
        assert_eq!(code.len(), 64);
        assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic, and exactly the machine pubkey's hex.
        assert_eq!(Some(code.clone()), device_pairing_code(&secret));
        let expected = PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret).unwrap()).to_hex();
        assert_eq!(code, expected);
    }

    #[test]
    fn parse_device_pub_accepts_valid_hex_trims_newline_and_roundtrips() {
        let code = device_pairing_code(&[7u8; 32]).unwrap();
        // Round-trips through the file format (trailing newline, as written).
        assert_eq!(parse_device_pub(&format!("{code}\n")), Ok(code.clone()));
        assert_eq!(
            parse_device_pub(&format!("  {}  ", code.to_uppercase())),
            Ok(code)
        );
    }

    #[test]
    fn parse_device_pub_rejects_empty_and_malformed() {
        assert_eq!(parse_device_pub(""), Err(DeviceCodeError::Empty));
        assert_eq!(parse_device_pub("   \n"), Err(DeviceCodeError::Empty));
        assert_eq!(
            parse_device_pub(&"a".repeat(63)),
            Err(DeviceCodeError::Malformed)
        );
        assert_eq!(
            parse_device_pub(&"a".repeat(65)),
            Err(DeviceCodeError::Malformed)
        );
        assert_eq!(
            parse_device_pub(&format!("{}z", "a".repeat(63))),
            Err(DeviceCodeError::Malformed)
        );
    }

    #[test]
    fn grouped_for_display_is_eight_eights_and_strips_back_to_raw() {
        let code = device_pairing_code(&[7u8; 32]).unwrap();
        let grouped = grouped_for_display(&code);
        let parts: Vec<&str> = grouped.split(' ').collect();
        assert_eq!(parts.len(), 8);
        assert!(parts.iter().all(|p| p.len() == 8));
        assert_eq!(grouped.replace(' ', ""), code);
    }
}
