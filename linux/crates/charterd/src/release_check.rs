//! D2: the laptop learns about its own updates from **signed relay events**
//! and stages the hash-verified `.deb` locally — no website in the loop.
//!
//! What this does NOT do: install. apt has no Blossom, and charterd silently
//! self-replacing is not the model — the staged file plus a journald line
//! hands the parent the same one-command install they do today, sourced
//! decentralized. The guardian console shows the "behind" notice from the
//! same event.
//!
//! Trust: `verify_software_release` against [`RELEASE_PUBKEY_HEX`] (compiled
//! in — a NEW trust-anchor category: every other pin is learned at pair
//! time). Downgrade safety: strictly-greater `version_code` only. Transport
//! failures are "no update news this round", never a crash.

use std::fs;
use std::path::{Path, PathBuf};

use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_sys::relay::{Filter, RelayTransport};
use charter_verify::software_release::{verify_software_release, SoftwareRelease};

/// x-only pubkey of the release key (ceremony 2026-08-12; see
/// android/keystore/README.md "The Nostr release key").
pub const RELEASE_PUBKEY_HEX: &str =
    "11ecfbc95f61796b0b0c5156a24edb324b0ea5e69ff659990b99ebe2f44043a0";

/// Release announcements ride trotters PLUS public relays — trotters must
/// never be load-bearing (the end state runs no services of ours). Keep in
/// sync with apps/charter-app/src/release/releaseTrust.ts.
pub const RELEASE_RELAYS: &[&str] = &[
    "wss://relay.trotters.cc",
    "wss://relay.damus.io",
    "wss://nos.lol",
];

/// This platform's channel (the event's `d` tag).
pub const CHANNEL: &str = "charter-deb";

/// Where the verified artifact lands, next to charterd's other state.
pub const UPDATES_DIR: &str = "/var/lib/charter/updates";

/// A `.deb` is ~10 MB; anything claiming more than this is refused before a
/// byte is fetched (the event names `size_bytes`, and the download re-checks).
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// First check a few minutes after boot, then every six hours. Update news
/// does not need enforcement-loop cadence.
pub const FIRST_CHECK_DELAY_SECS: u64 = 300;
pub const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// The pinned release key, parsed. Panics only on a corrupted binary — the
/// constant is compile-time.
pub fn pinned_release_key() -> PubKey {
    PubKey::from_hex(RELEASE_PUBKEY_HEX).expect("compiled-in release pubkey is valid hex")
}

/// What got staged, for logging/consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    pub version_name: String,
    pub version_code: u64,
    pub path: PathBuf,
}

/// The newest verified announcement STRICTLY newer than what's installed.
/// Relays may disagree about "latest" (addressable events replace per-relay),
/// so every candidate is verified and compared — replaceable semantics are
/// never trusted.
pub fn select_release(
    events: &[NostrEvent],
    pinned: &PubKey,
    installed_code: u64,
) -> Option<SoftwareRelease> {
    let mut best: Option<SoftwareRelease> = None;
    for ev in events {
        let Ok(r) = verify_software_release(ev, pinned, CHANNEL) else {
            continue;
        };
        if r.version_code <= installed_code {
            continue;
        }
        if r.size_bytes > MAX_ARTIFACT_BYTES {
            eprintln!(
                "charterd: release {} claims {} bytes (> {} cap) — refused",
                r.version_name, r.size_bytes, MAX_ARTIFACT_BYTES
            );
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|b| r.version_code > b.version_code)
        {
            best = Some(r);
        }
    }
    best
}

/// The staged path a release would land at (stable per version).
pub fn staged_path(dir: &Path, r: &SoftwareRelease) -> PathBuf {
    dir.join(format!("charter_{}_amd64.deb", r.version_name))
}

/// Verify `bytes` against the announcement and write them at the staged path
/// via `.part` + rename — unverified bytes never sit at the final name.
/// Returns None on hash/size mismatch (poisoned or truncated download).
pub fn stage_bytes(bytes: &[u8], r: &SoftwareRelease, dir: &Path) -> Option<StagedUpdate> {
    if bytes.len() as u64 != r.size_bytes {
        eprintln!(
            "charterd: update {}: downloaded {} bytes, event says {} — dropped",
            r.version_name,
            bytes.len(),
            r.size_bytes
        );
        return None;
    }
    let got = charter_crypto::sha256(bytes);
    if got != *r.sha256.as_bytes() {
        eprintln!(
            "charterd: update {}: download didn't match its pinned sha256 — dropped",
            r.version_name
        );
        return None;
    }
    if fs::create_dir_all(dir).is_err() {
        return None;
    }
    let dest = staged_path(dir, r);
    let part = dest.with_extension("deb.part");
    if fs::write(&part, bytes).is_err() || fs::rename(&part, &dest).is_err() {
        let _ = fs::remove_file(&part);
        return None;
    }
    // A tiny marker consumers (console Today page, future tray line) can read
    // without dpkg-parsing the artifact.
    let ready = serde_json::json!({
        "versionName": r.version_name,
        "versionCode": r.version_code,
        "sha256": r.sha256.to_hex(),
        "path": dest.to_string_lossy(),
    });
    let _ = fs::write(dir.join("ready.json"), ready.to_string());
    Some(StagedUpdate {
        version_name: r.version_name.clone(),
        version_code: r.version_code,
        path: dest,
    })
}

/// One full round: query → verify/select → (skip if already staged) → fetch
/// from mirrors in order → stage. `fetch` is injected so tests never touch a
/// network (the real one is [`real::fetch_blob`]); `pinned` is
/// [`pinned_release_key`] in production and a test key in tests.
pub async fn check_and_stage<T, F, Fut>(
    transport: &T,
    pinned: &PubKey,
    installed_code: u64,
    dir: &Path,
    fetch: F,
) -> Option<StagedUpdate>
where
    T: RelayTransport + ?Sized,
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let relays: Vec<String> = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
    let filter = Filter {
        kinds: vec![kinds::SOFTWARE_RELEASE],
        authors: vec![*pinned],
        ..Filter::default()
    };
    let events = transport.query(&relays, filter).await.unwrap_or_default();
    let r = select_release(&events, pinned, installed_code)?;
    let dest = staged_path(dir, &r);
    if dest.is_file() {
        // Same version already staged and verified in a prior round.
        return None;
    }
    for url in &r.urls {
        match fetch(url.clone()).await {
            Ok(bytes) => {
                if let Some(staged) = stage_bytes(&bytes, &r, dir) {
                    eprintln!(
                        "charterd: update {} staged at {} — install with: \
                         sudo apt install {}",
                        staged.version_name,
                        staged.path.display(),
                        staged.path.display()
                    );
                    return Some(staged);
                }
                // Hash mismatch from THIS mirror — try the next one.
            }
            Err(why) => {
                eprintln!("charterd: update mirror {url}: {why}");
            }
        }
    }
    None
}

#[cfg(feature = "real")]
pub mod real {
    //! The reqwest half + the background task. Kept apart so the logic above
    //! tests under `mock` without an HTTP client in the graph.

    use super::*;
    use charter_sys::relay::RealRelayTransport;

    /// GET one mirror. Mirrors must answer 200 directly — redirects are
    /// refused, matching the Android stagers (and the publisher's verify).
    pub async fn fetch_blob(url: String) -> Result<Vec<u8>, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;
        let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
        if res.status() != reqwest::StatusCode::OK {
            return Err(format!("HTTP {}", res.status()));
        }
        if let Some(len) = res.content_length() {
            if len > MAX_ARTIFACT_BYTES {
                return Err(format!("{len} bytes exceeds the artifact cap"));
            }
        }
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err("body exceeds the artifact cap".into());
        }
        Ok(bytes.to_vec())
    }

    /// Background task: first check shortly after boot, then every
    /// [`CHECK_INTERVAL_SECS`]. Its own transport + task, like the pair
    /// listener — update news must never delay an enforcement tick.
    pub fn spawn_release_check() {
        tokio::spawn(async move {
            let transport = RealRelayTransport::default();
            let dir = PathBuf::from(UPDATES_DIR);
            tokio::time::sleep(std::time::Duration::from_secs(FIRST_CHECK_DELAY_SECS)).await;
            let pinned = pinned_release_key();
            loop {
                let installed = crate::version::version_code();
                let _ = check_and_stage(&transport, &pinned, installed, &dir, fetch_blob).await;
                tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
            }
        });
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use charter_sys::relay::MockRelayTransport;
    use charter_sys::signer::{MachineSigner, SeedSigner};

    const NOW: u64 = 1_754_900_000;

    fn signer() -> SeedSigner {
        SeedSigner::from_seed(0x11)
    }

    fn pin_of(signer: &SeedSigner) -> PubKey {
        MachineSigner::pubkey(signer)
    }

    fn release_event(signer: &SeedSigner, version_code: u64, bytes: &[u8]) -> NostrEvent {
        let sha = charter_primitives::Sha256Hex::from_bytes(charter_crypto::sha256(bytes));
        let tags = vec![
            vec!["d".into(), CHANNEL.into()],
            vec!["version".into(), format!("0.7.{version_code}")],
            vec!["version_code".into(), version_code.to_string()],
            vec!["x".into(), sha.to_hex()],
            vec!["size".into(), bytes.len().to_string()],
            vec![
                "url".into(),
                format!("https://blossom.example/{}", sha.to_hex()),
            ],
        ];
        sign_release(signer, tags)
    }

    fn sign_release(signer: &SeedSigner, tags: Vec<Vec<String>>) -> NostrEvent {
        // Local sign_event equivalent (charter-verify's test_support is not a
        // public dep here): id then sig, deterministic signer.
        let mut ev = NostrEvent {
            id: charter_primitives::EventId::from_bytes([0; 32]),
            pubkey: MachineSigner::pubkey(signer),
            created_at: NOW,
            kind: kinds::SOFTWARE_RELEASE,
            tags,
            content: String::new(),
            sig: charter_primitives::Sig::from_bytes([0; 64]),
        };
        ev.id = charter_verify::event::event_id(&ev);
        ev.sig = MachineSigner::sign(signer, ev.id.as_bytes()).expect("sign");
        ev
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "charterd-release-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn select_picks_newest_verified_and_never_downgrades() {
        let s = signer();
        let pin = pin_of(&s);
        let bytes = b"deb bytes".to_vec();
        let evs = vec![
            release_event(&s, 707, &bytes),
            release_event(&s, 710, &bytes),
            release_event(&SeedSigner::from_seed(0x99), 999, &bytes), // wrong signer
        ];
        let r = select_release(&evs, &pin, 706).expect("newer release");
        assert_eq!(r.version_code, 710);
        // Equal or older than installed: nothing.
        assert!(select_release(&evs, &pin, 710).is_none());
        assert!(select_release(&evs, &pin, 800).is_none());
    }

    #[test]
    fn select_refuses_an_oversize_claim() {
        let s = signer();
        let pin = pin_of(&s);
        let sha = charter_primitives::Sha256Hex::from_bytes([0xab; 32]);
        let tags = vec![
            vec!["d".into(), CHANNEL.into()],
            vec!["version".into(), "9.9.9".into()],
            vec!["version_code".into(), "90909".into()],
            vec!["x".into(), sha.to_hex()],
            vec!["size".into(), (MAX_ARTIFACT_BYTES + 1).to_string()],
            vec![
                "url".into(),
                format!("https://blossom.example/{}", sha.to_hex()),
            ],
        ];
        let ev = sign_release(&s, tags);
        assert!(select_release(&[ev], &pin, 0).is_none());
    }

    #[test]
    fn stage_bytes_pins_hash_and_size_and_writes_ready_json() {
        let s = signer();
        let bytes = b"real deb bytes".to_vec();
        let ev = release_event(&s, 707, &bytes);
        let r = verify_software_release(&ev, &pin_of(&s), CHANNEL).unwrap();
        let dir = tmpdir();

        // Wrong bytes: refused, nothing at the final name.
        assert!(stage_bytes(b"evil bytes!!!!", &r, &dir).is_none());
        assert!(!staged_path(&dir, &r).exists());

        // Right bytes: staged + marker.
        let staged = stage_bytes(&bytes, &r, &dir).expect("staged");
        assert_eq!(staged.version_code, 707);
        assert_eq!(fs::read(&staged.path).unwrap(), bytes);
        let ready = fs::read_to_string(dir.join("ready.json")).unwrap();
        assert!(ready.contains("\"versionCode\":707"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A two-mirror event: the FIRST url 404s, the second serves the bytes.
    fn two_mirror_event(signer: &SeedSigner, version_code: u64, bytes: &[u8]) -> NostrEvent {
        let sha = charter_primitives::Sha256Hex::from_bytes(charter_crypto::sha256(bytes));
        let tags = vec![
            vec!["d".into(), CHANNEL.into()],
            vec!["version".into(), format!("0.7.{version_code}")],
            vec!["version_code".into(), version_code.to_string()],
            vec!["x".into(), sha.to_hex()],
            vec!["size".into(), bytes.len().to_string()],
            vec!["url".into(), "https://dead.example/blob".into()],
            vec![
                "url".into(),
                format!("https://blossom.example/{}", sha.to_hex()),
            ],
        ];
        sign_release(signer, tags)
    }

    #[tokio::test]
    async fn check_and_stage_full_round_with_mirror_fallback() {
        let s = signer();
        let pin = pin_of(&s);
        let bytes = b"the artifact".to_vec();
        let relay = MockRelayTransport::new();
        let relays: Vec<String> = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
        let _ = relay
            .publish(&relays, two_mirror_event(&s, 720, &bytes))
            .await;
        let dir = tmpdir();

        let good = bytes.clone();
        let fetch = move |url: String| {
            let good = good.clone();
            async move {
                if url.contains("dead.example") {
                    Err("HTTP 404".to_string())
                } else {
                    Ok(good.clone())
                }
            }
        };

        let staged = check_and_stage(&relay, &pin, 706, &dir, fetch.clone())
            .await
            .expect("staged via the fallback mirror");
        assert_eq!(staged.version_code, 720);
        assert!(staged.path.is_file());
        assert_eq!(fs::read(&staged.path).unwrap(), bytes);

        // Second round, same version: already staged → no re-download.
        assert!(check_and_stage(&relay, &pin, 706, &dir, fetch)
            .await
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn check_and_stage_ignores_a_forged_announcement() {
        let s = SeedSigner::from_seed(0x99); // NOT the pin
        let pin = pin_of(&signer());
        let bytes = b"evil".to_vec();
        let relay = MockRelayTransport::new();
        let relays: Vec<String> = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
        let _ = relay
            .publish(&relays, two_mirror_event(&s, 999, &bytes))
            .await;
        let dir = tmpdir();
        let fetch = |_url: String| async { Ok(b"evil".to_vec()) };
        assert!(check_and_stage(&relay, &pin, 0, &dir, fetch)
            .await
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
