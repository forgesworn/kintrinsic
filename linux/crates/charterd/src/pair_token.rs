//! The ward's one-time pairing token: minted while an unpaired ward shows its
//! pairing QR, spent the moment a guardian's offer proves knowledge of it.
//!
//! Knowing this token means having physically looked at the ward's screen —
//! the same trust basis as typing on the machine itself, which is why it is
//! enough to authorise a pin.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// How long a shown token stays valid. Long enough to fetch a phone from the
/// next room, short enough that a screen glimpsed in passing goes stale.
pub const TOKEN_TTL_SECS: u64 = 600;

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
    minted_at: u64,
}

/// Mint + persist a fresh token, returning its hex. Overwrites any previous
/// one, so re-opening the pairing screen always invalidates the old QR.
pub fn mint(path: &str, now: u64, random: [u8; 16]) -> std::io::Result<String> {
    let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string(&Stored {
        token: token.clone(),
        minted_at: now,
    })
    .expect("token serializes");
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, body)?;
    // 0600 — unlike the pairing pin, this IS a secret: it is the sole proof of
    // physical presence, so only root may read it.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(token)
}

/// The live token, or `None` when absent, unreadable, junk, or expired.
pub fn current(path: &str, now: u64) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let stored: Stored = serde_json::from_str(&text).ok()?;
    if now.saturating_sub(stored.minted_at) >= TOKEN_TTL_SECS {
        return None;
    }
    Some(stored.token)
}

/// Spend the token — single use, so a captured offer replayed later finds
/// nothing to match against.
pub fn consume(path: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("charter-token-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("pair-token.json").to_string_lossy().into_owned()
    }

    #[test]
    fn a_minted_token_reads_back_until_it_expires() {
        let p = tmp("mint");
        let t = mint(&p, 1_000, [0xAB; 16]).unwrap();
        assert_eq!(t.len(), 32);
        assert_eq!(current(&p, 1_000).as_deref(), Some(t.as_str()));
        assert_eq!(
            current(&p, 1_000 + TOKEN_TTL_SECS - 1).as_deref(),
            Some(t.as_str())
        );
        assert!(current(&p, 1_000 + TOKEN_TTL_SECS).is_none());
    }

    #[test]
    fn minting_again_invalidates_the_previous_token() {
        let p = tmp("remint");
        let first = mint(&p, 1_000, [0x01; 16]).unwrap();
        let second = mint(&p, 1_010, [0x02; 16]).unwrap();
        assert_ne!(first, second);
        assert_eq!(current(&p, 1_010).as_deref(), Some(second.as_str()));
    }

    #[test]
    fn consume_makes_it_unusable() {
        let p = tmp("consume");
        mint(&p, 1_000, [0x11; 16]).unwrap();
        consume(&p).unwrap();
        assert!(current(&p, 1_000).is_none());
        // Consuming twice is not an error — the loop may race itself.
        consume(&p).unwrap();
    }

    #[test]
    fn absent_or_junk_token_file_is_simply_no_token() {
        let p = tmp("junk");
        assert!(current(&p, 1_000).is_none());
        std::fs::write(&p, b"not json").unwrap();
        assert!(current(&p, 1_000).is_none());
    }

    #[test]
    fn the_token_file_is_not_world_readable() {
        let p = tmp("perms");
        mint(&p, 1_000, [0x33; 16]).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the token is a secret, unlike the pairing pin");
    }
}
