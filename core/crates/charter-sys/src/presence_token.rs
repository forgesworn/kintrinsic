//! The ward's one-time presence token (contract, *Presence confirmation*):
//! minted only on a privileged request at the ward (an administrator on
//! Linux, the guardian PIN on Android), shown as a code to scan, and spent by
//! the presence confirmation that echoes it. Token rules are scan-to-pair's:
//! 16 random bytes, a 600-second lifetime, single use, compared in constant
//! time.
//!
//! Knowing the token means having physically looked at the ward's screen, so
//! the file is a secret: it is created `0600` at open, never chmodded after a
//! write. Shared by the Linux warden and the Android ward; std only, so it
//! builds under every `charter-sys` feature set.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long a minted token stays valid (scan-to-pair's lifetime).
pub const PRESENCE_TOKEN_TTL_SECS: u64 = 600;

/// Random bytes in a token.
pub const PRESENCE_TOKEN_BYTES: usize = 16;

/// A live token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceToken {
    /// 32 lowercase hex characters.
    pub token: String,
    /// Ward time it was minted.
    pub minted_at: u64,
    /// The `d` of the one pending statement it may confirm, if it was minted
    /// for one; `None` leaves it open (open question "Presence confirmation
    /// details").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_to: Option<String>,
}

/// The token file, e.g. `<state dir>/presence-token.json`.
#[derive(Debug, Clone)]
pub struct PresenceTokenStore {
    path: PathBuf,
}

impl PresenceTokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        PresenceTokenStore { path: path.into() }
    }

    /// Mint and persist a fresh token from 16 bytes `fill` provides (the
    /// caller's RNG), replacing any previous one, so showing a new code always
    /// invalidates the old one. The CALLER is responsible for the privilege
    /// check that must come first.
    pub fn mint(
        &self,
        now: u64,
        bound_to: Option<&str>,
        fill: &mut dyn FnMut(&mut [u8]),
    ) -> std::io::Result<PresenceToken> {
        let mut random = [0u8; PRESENCE_TOKEN_BYTES];
        fill(&mut random);
        let token = PresenceToken {
            token: random.iter().map(|b| format!("{b:02x}")).collect(),
            minted_at: now,
            bound_to: bound_to.map(str::to_owned),
        };
        let body = serde_json::to_string(&token).map_err(std::io::Error::other)?;
        write_secret(&self.path, body.as_bytes())?;
        Ok(token)
    }

    /// The live token, or `None` when absent, unreadable, junk, or expired
    /// (also when minted in the future: a clock set back cannot stretch a
    /// token's life).
    pub fn current(&self, now: u64) -> Option<PresenceToken> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let stored: PresenceToken = serde_json::from_str(&text).ok()?;
        if stored.minted_at > now || now - stored.minted_at >= PRESENCE_TOKEN_TTL_SECS {
            return None;
        }
        Some(stored)
    }

    /// The live token if `candidate` matches it, compared in constant time.
    /// Does not spend it: a token is spent only by a successful confirmation
    /// (see [`Self::consume`]).
    pub fn matching(&self, candidate: &str, now: u64) -> Option<PresenceToken> {
        let live = self.current(now)?;
        constant_time_eq(live.token.as_bytes(), candidate.as_bytes()).then_some(live)
    }

    /// Spend the token: single use, so a captured confirmation replayed later
    /// finds nothing to match. Absent is not an error.
    pub fn consume(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    /// [`Self::matching`] and, on a match, [`Self::consume`].
    pub fn consume_if_matches(
        &self,
        candidate: &str,
        now: u64,
    ) -> std::io::Result<Option<PresenceToken>> {
        match self.matching(candidate, now) {
            Some(t) => {
                self.consume()?;
                Ok(Some(t))
            }
            None => Ok(None),
        }
    }
}

/// Compare without an early exit, so the token cannot be walked out byte by
/// byte with timing. A length mismatch is a non-match.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Write `body` to `path` via a fresh `0600` temp file and a rename. The mode
/// is set AT OPEN on a file this call created: a write-then-chmod would leave
/// the token readable under a 022 umask for the length of the write. A
/// leftover temp file is removed first so `create_new` never inherits an old
/// file's mode or an open reader.
fn write_secret(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (PathBuf, PresenceTokenStore) {
        let d =
            std::env::temp_dir().join(format!("charter-presence-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = PresenceTokenStore::new(d.join(name).join("presence-token.json"));
        (d, s)
    }

    fn fill_with(b: u8) -> impl FnMut(&mut [u8]) {
        move |buf: &mut [u8]| buf.fill(b)
    }

    #[test]
    fn a_minted_token_is_live_for_600_seconds() {
        let (_d, s) = store("mint");
        let t = s.mint(1_000, None, &mut fill_with(0xab)).unwrap();
        assert_eq!(t.token, "ab".repeat(16));
        assert_eq!(s.current(1_000), Some(t.clone()));
        assert_eq!(s.current(1_000 + PRESENCE_TOKEN_TTL_SECS - 1), Some(t));
        assert!(s.current(1_000 + PRESENCE_TOKEN_TTL_SECS).is_none());
        assert!(s.current(999).is_none(), "minted in the future");
    }

    #[test]
    fn a_match_spends_it_and_a_mismatch_does_not() {
        let (_d, s) = store("spend");
        let t = s
            .mint(1_000, Some("kintrinsic/guardian/ab"), &mut fill_with(0x11))
            .unwrap();
        assert!(s
            .consume_if_matches(&"22".repeat(16), 1_001)
            .unwrap()
            .is_none());
        assert!(
            s.matching(&t.token, 1_001).is_some(),
            "a miss spends nothing"
        );
        let got = s.consume_if_matches(&t.token, 1_002).unwrap().unwrap();
        assert_eq!(got.bound_to.as_deref(), Some("kintrinsic/guardian/ab"));
        assert!(s.consume_if_matches(&t.token, 1_003).unwrap().is_none());
        s.consume().unwrap();
    }

    #[test]
    fn minting_again_replaces_the_previous_token() {
        let (_d, s) = store("remint");
        let first = s.mint(1_000, None, &mut fill_with(1)).unwrap();
        let second = s.mint(1_010, None, &mut fill_with(2)).unwrap();
        assert!(s.matching(&first.token, 1_010).is_none());
        assert!(s.matching(&second.token, 1_010).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let (d, s) = store("perms");
        s.mint(1_000, None, &mut fill_with(3)).unwrap();
        let p = d.join("perms").join("presence-token.json");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
