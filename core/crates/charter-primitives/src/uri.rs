//! Minimal percent-decoding for pairing-URI query values.
//!
//! The `bunker://` contract grammar wants a literal `relay=wss://…`, but
//! guardian apps built on browser stacks historically emitted
//! `relay=wss%3A%2F%2F…` (`encodeURIComponent`), producing URIs no device
//! could pin. Decoding at the consumer repairs every such link already in the
//! field — while a value that was never encoded passes through unchanged, so
//! both forms pin. Single-sourced here because BOTH the device pin
//! (`charter-transport`) and the CLI validator (`charter-cli`) must move in
//! lockstep — a decoder gap between them is exactly the class of late-failure
//! bug this exists to close.

/// Decode `%XX` escapes in a URI component. Malformed escapes (a `%` not
/// followed by two hex digits) pass through literally — the caller's
/// validation (e.g. the `wss://` prefix check) then rejects garbage; nothing
/// is ever silently dropped. `+` is NOT treated as a space: pairing URIs are
/// `encodeURIComponent`-style, not form-encoded.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(hi), Some(lo)) = (hex_val(bytes.get(i + 1)), hex_val(bytes.get(i + 2))) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // Pairing URIs are ASCII in practice; lossy conversion never corrupts a
    // valid one, and an invalid one fails the caller's checks anyway.
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: Option<&u8>) -> Option<u8> {
    match b {
        Some(c @ b'0'..=b'9') => Some(c - b'0'),
        Some(c @ b'a'..=b'f') => Some(c - b'a' + 10),
        Some(c @ b'A'..=b'F') => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decodes_encodeuricomponent_wss_url() {
        assert_eq!(
            percent_decode("wss%3A%2F%2Frelay.trotters.cc"),
            "wss://relay.trotters.cc"
        );
    }

    #[test]
    fn plain_value_passes_through() {
        assert_eq!(
            percent_decode("wss://relay.trotters.cc"),
            "wss://relay.trotters.cc"
        );
    }

    #[test]
    fn uppercase_hex_decodes() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("a%2fb"), "a/b");
    }

    #[test]
    fn malformed_escape_passes_through_literally() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%2"), "%2");
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(percent_decode(""), "");
    }
}
