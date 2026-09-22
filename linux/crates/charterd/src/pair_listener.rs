//! The unpaired ward's narrow inbound door.
//!
//! A paired ward talks to its pinned guardian. An **unpaired** one previously
//! talked to nobody at all — which is why scanning its QR could never finish
//! the job: the phone learned the laptop's address, and the laptop learned
//! nothing. This opens exactly one door, and only while a freshly-shown QR's
//! token is still live: PAIR_OFFERs addressed to this machine. Everything else
//! an unpaired device might be sent on a public relay is ignored.

use charter_primitives::PubKey;
use charter_sys::relay::RelayUrl;

use crate::pair_accept::choose_offer;
use crate::pair_commit::{commit_pin, PinPaths};

/// How often the unpaired ward checks for an offer while its QR is on screen.
/// Fast enough that "point the phone at it" feels immediate.
pub const POLL_INTERVAL_SECS: u64 = 3;

/// Relays an unpaired ward listens on. It has no pinned pairing yet, so it
/// cannot learn these from one — they are the same default Kintrinsic publishes
/// to (`apps/charter-app/src/signer/config.ts`). `CHARTER_PAIR_RELAYS`
/// (comma-separated) overrides for a self-hosted relay.
pub const DEFAULT_PAIR_RELAYS: &[&str] = &["wss://relay.trotters.cc"];

/// The relay set to listen on, honouring the override.
pub fn pair_relays() -> Vec<RelayUrl> {
    match std::env::var("CHARTER_PAIR_RELAYS") {
        Ok(s) => {
            let out: Vec<RelayUrl> = s
                .split(',')
                .map(|r| r.trim().to_string())
                .filter(|r| r.starts_with("wss://"))
                .collect();
            if out.is_empty() {
                default_pair_relays()
            } else {
                out
            }
        }
        Err(_) => default_pair_relays(),
    }
}

fn default_pair_relays() -> Vec<RelayUrl> {
    DEFAULT_PAIR_RELAYS.iter().map(|r| r.to_string()).collect()
}

/// One pass: given the offers polled this tick, pin a guardian if exactly one
/// proves the live token.
///
/// Returns the pinned key, or `None` to keep waiting. A refused commit is
/// logged and swallowed — a ward that cannot pair must keep enforcing the
/// limits it already has, never fall over.
pub fn try_pair_once(
    paths: &PinPaths,
    machine: PubKey,
    offers: &[charter_transport::transport::ReceivedPairOffer],
    subject_random: [u8; 32],
    now: u64,
) -> Option<PubKey> {
    let token = crate::pair_token::current(&paths.token, now)?;
    let guardian = choose_offer(offers, &token, now)?;
    // Listen on the relays the winning offer actually named, so a self-hosted
    // guardian keeps working after the pin.
    let relays: Vec<RelayUrl> = offers
        .iter()
        .find(|o| o.seal_author == guardian)
        .map(|o| o.offer.relays.clone())
        .unwrap_or_else(pair_relays);

    match commit_pin(paths, guardian, &relays, machine, subject_random, now) {
        Ok(()) => {
            eprintln!(
                "charterd: paired by scan — pinned guardian {}",
                &guardian.to_hex()[..8]
            );
            Some(guardian)
        }
        Err(e) => {
            eprintln!("charterd: pairing offer refused — {e}");
            None
        }
    }
}

/// Draw a fresh subject from the OS CSPRNG. `None` if `/dev/urandom` is
/// somehow unreadable, which simply defers pairing to the next tick.
pub fn draw_subject() -> Option<[u8; 32]> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_transport::pair_offer::PairOffer;
    use charter_transport::transport::ReceivedPairOffer;

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

    fn fixture(name: &str) -> PinPaths {
        let d = std::env::temp_dir().join(format!("charter-listen-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("limits.d")).unwrap();
        std::fs::write(
            d.join("limits.d/axel.json"),
            r#"{"limits":{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}"#,
        )
        .unwrap();
        PinPaths {
            pairing: d.join("pairing.json").to_string_lossy().into_owned(),
            limits_dir: d.join("limits.d").to_string_lossy().into_owned(),
            token: d.join("pair-token.json").to_string_lossy().into_owned(),
            children_base: d.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn a_ward_showing_its_qr_pins_the_guardian_that_proves_the_token() {
        let paths = fixture("ok");
        let token = crate::pair_token::mint(&paths.token, 100, [0x33; 16]).unwrap();

        let pinned = try_pair_once(
            &paths,
            PubKey::from_bytes([0xBB; 32]),
            &[offer(&token, 100, 0xAA)],
            [0xCC; 32],
            100,
        );

        assert_eq!(pinned, Some(PubKey::from_bytes([0xAA; 32])));
        assert!(std::path::Path::new(&paths.pairing).exists());
    }

    #[test]
    fn a_ward_with_no_qr_on_screen_pins_nobody() {
        // No token minted: the pairing screen was never opened, so an offer
        // arriving out of the blue must be ignored entirely.
        let paths = fixture("notoken");
        let pinned = try_pair_once(
            &paths,
            PubKey::from_bytes([0xBB; 32]),
            &[offer("abababababababababababababababab", 100, 0xAA)],
            [0xCC; 32],
            100,
        );
        assert_eq!(pinned, None);
        assert!(!std::path::Path::new(&paths.pairing).exists());
    }

    #[test]
    fn a_stranger_guessing_the_wrong_token_pins_nobody() {
        let paths = fixture("wrong");
        crate::pair_token::mint(&paths.token, 100, [0x33; 16]).unwrap();
        let pinned = try_pair_once(
            &paths,
            PubKey::from_bytes([0xBB; 32]),
            &[offer("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd", 100, 0xAA)],
            [0xCC; 32],
            100,
        );
        assert_eq!(pinned, None);
        assert!(!std::path::Path::new(&paths.pairing).exists());
    }

    #[test]
    fn the_default_relay_matches_mycharter() {
        std::env::remove_var("CHARTER_PAIR_RELAYS");
        assert_eq!(pair_relays(), vec!["wss://relay.trotters.cc".to_string()]);
    }

    #[test]
    fn a_junk_relay_override_falls_back_to_the_default() {
        // An insecure ws:// override must never silently downgrade the pairing
        // channel — fall back rather than listen in the clear.
        std::env::set_var("CHARTER_PAIR_RELAYS", "ws://nope,http://also-nope");
        assert_eq!(pair_relays(), vec!["wss://relay.trotters.cc".to_string()]);
        std::env::remove_var("CHARTER_PAIR_RELAYS");
    }
}
