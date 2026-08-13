//! The PAIR_OFFER payload (kind 31117): a guardian's offer to be pinned by an
//! unpaired ward, carrying the one-time token from the ward's on-screen QR.
//!
//! SECURITY: this payload deliberately carries **no guardian pubkey**. The
//! device pins the authenticated seal author instead, so a forged payload can
//! only ever nominate the sender themselves — which is the whole point.

use serde::{Deserialize, Serialize};

use charter_sys::relay::RelayUrl;

/// The one-time pairing token: 16 random bytes, lowercase hex.
pub const TOKEN_HEX_LEN: usize = 32;

/// A validated offer to pin a guardian.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairOffer {
    pub token: String,
    pub relays: Vec<RelayUrl>,
    pub ts: u64,
}

#[derive(Deserialize)]
struct RawOffer {
    token: String,
    relays: Vec<String>,
    ts: u64,
}

fn valid_token(t: &str) -> bool {
    t.len() == TOKEN_HEX_LEN && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse + validate an offer. Refuses a malformed token, or an offer with no
/// `wss://` relay — the same secure-transport floor the `bunker://` pin uses.
pub fn parse_pair_offer(json: &str) -> Option<PairOffer> {
    let raw: RawOffer = serde_json::from_str(json).ok()?;
    if !valid_token(&raw.token) {
        return None;
    }
    let relays: Vec<RelayUrl> = raw
        .relays
        .into_iter()
        .filter(|r| r.starts_with("wss://"))
        .collect();
    if relays.is_empty() {
        return None;
    }
    Some(PairOffer {
        token: raw.token.to_ascii_lowercase(),
        relays,
        ts: raw.ts,
    })
}

/// Serialize an offer for the inner rumor's `content`.
pub fn build_pair_offer(token: &str, relays: &[RelayUrl], ts: u64) -> String {
    let offer = PairOffer {
        token: token.to_string(),
        relays: relays.to_vec(),
        ts,
    };
    serde_json::to_string(&offer).expect("offer serializes")
}

/// Constant-time token comparison — never short-circuits on the first
/// differing byte, so a network attacker cannot walk the token out of the
/// device one byte at a time with timing.
pub fn token_matches(expected: &str, got: &str) -> bool {
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relays() -> Vec<RelayUrl> {
        vec!["wss://relay.trotters.cc".to_string()]
    }

    #[test]
    fn round_trips_an_offer() {
        let token = "ab".repeat(16);
        let json = build_pair_offer(&token, &relays(), 1234);
        let got = parse_pair_offer(&json).unwrap();
        assert_eq!(got.token, token);
        assert_eq!(got.relays, relays());
        assert_eq!(got.ts, 1234);
    }

    #[test]
    fn rejects_a_token_that_is_not_32_hex() {
        for bad in ["abc", &"zz".repeat(16), "", &"ab".repeat(20)] {
            let json =
                format!(r#"{{"token":"{bad}","relays":["wss://relay.trotters.cc"],"ts":1}}"#);
            assert!(parse_pair_offer(&json).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_an_offer_with_no_secure_relay() {
        let json = r#"{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","relays":["ws://nope"],"ts":1}"#;
        assert!(parse_pair_offer(json).is_none());
    }

    #[test]
    fn token_compare_is_exact() {
        let t = "a".repeat(32);
        assert!(token_matches(&t, &t));
        assert!(!token_matches(&t, &"b".repeat(32)));
        assert!(!token_matches(&t, &"a".repeat(31)));
        assert!(!token_matches(&t, ""));
    }
}
