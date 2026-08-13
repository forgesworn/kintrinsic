//! Who — if anyone — may pin this ward. A pure decision with no I/O, so every
//! refusal path is covered by a unit test rather than by hope.

use charter_primitives::PubKey;
use charter_transport::pair_offer::token_matches;
use charter_transport::transport::ReceivedPairOffer;

use crate::pair_token::TOKEN_TTL_SECS;

/// The guardian key to pin, or `None` to keep waiting.
///
/// Refuses:
/// - a mismatched token (the common case: someone else's offer),
/// - an offer older than the token's own TTL, or dated in the future — a
///   captured-and-replayed offer, or one forged against a skewed clock,
/// - and, deliberately, a **tie**: two distinct keys both presenting the
///   token. A tie means the token leaked, and pinning either would be a coin
///   flip on who owns the ward. Refusing costs the parent one retry; guessing
///   could hand a child's laptop to a stranger.
pub fn choose_offer(
    offers: &[ReceivedPairOffer],
    expected_token: &str,
    now: u64,
) -> Option<PubKey> {
    let mut winner: Option<PubKey> = None;
    for o in offers {
        if !token_matches(expected_token, &o.offer.token) {
            continue;
        }
        if o.offer.ts > now || now.saturating_sub(o.offer.ts) >= TOKEN_TTL_SECS {
            continue;
        }
        match winner {
            None => winner = Some(o.seal_author),
            Some(w) if w == o.seal_author => {}
            Some(_) => return None, // contested token — pin nobody
        }
    }
    winner
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_transport::pair_offer::PairOffer;

    fn offer(token: &str, ts: u64, who: u8) -> ReceivedPairOffer {
        ReceivedPairOffer {
            offer: PairOffer {
                token: token.to_string(),
                relays: vec!["wss://relay.trotters.cc".to_string()],
                ts,
            },
            seal_author: PubKey::from_bytes([who; 32]),
        }
    }

    const GOOD: &str = "abababababababababababababababab";

    #[test]
    fn accepts_the_offer_that_proves_the_token() {
        let got = choose_offer(&[offer(GOOD, 100, 0xAA)], GOOD, 100);
        assert_eq!(got, Some(PubKey::from_bytes([0xAA; 32])));
    }

    #[test]
    fn refuses_a_wrong_token() {
        let bad = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        assert!(choose_offer(&[offer(bad, 100, 0xAA)], GOOD, 100).is_none());
    }

    #[test]
    fn refuses_a_stale_offer() {
        let now = 100 + TOKEN_TTL_SECS + 1;
        assert!(choose_offer(&[offer(GOOD, 100, 0xAA)], GOOD, now).is_none());
    }

    #[test]
    fn refuses_a_future_dated_offer() {
        assert!(choose_offer(&[offer(GOOD, 10_000, 0xAA)], GOOD, 100).is_none());
    }

    #[test]
    fn refuses_outright_when_two_keys_race_the_same_token() {
        let both = [offer(GOOD, 100, 0xAA), offer(GOOD, 100, 0xBB)];
        assert!(
            choose_offer(&both, GOOD, 100).is_none(),
            "a contested token must pin nobody"
        );
    }

    #[test]
    fn one_guardian_offering_twice_is_not_a_tie() {
        // A retry from the same phone is ordinary, not an attack.
        let twice = [offer(GOOD, 100, 0xAA), offer(GOOD, 101, 0xAA)];
        assert_eq!(
            choose_offer(&twice, GOOD, 101),
            Some(PubKey::from_bytes([0xAA; 32]))
        );
    }

    #[test]
    fn an_empty_mailbox_pins_nobody() {
        assert!(choose_offer(&[], GOOD, 100).is_none());
    }
}
