//! The Linux warden's side of *Guardian roots and working keys (v1)*
//! (`spec/contract.md`): where the guardian-keys state lives, the presence
//! code an administrator shows at the ward, the offline-unlock codes for
//! every key in the signer set, and the world-readable view of who the ward
//! trusts and who set its rules (rule 5).
//!
//! The daemon is the ONE writer of the state document (`guardian-state.json`,
//! `0600`): it loads it once per run and hands the same adapter to the
//! broker, both transports, the local link, the lock and the STATUS feed.
//! A helper process never loads it for writing. `charter-presence` asks the
//! daemon to mint through a request file only root can create, and reads the
//! state with [`peek_keys`]; `charter-pair` clears it when it pins afresh.
//!
//! Everything here is pure or plain file IO, so it is built and tested under
//! both feature sets.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use charter_ipc::dto::{GuardiansView, PendingStatementView, RuleSetterView};
use charter_primitives::PubKey;
use charter_spine::{GuardianKeys, GuardianStatements, MintedToken, PresenceError};
use charter_sys::guardian_store::{FileGuardianStateStore, GUARDIAN_STATE_FILE};
use charter_sys::presence_token::PresenceTokenStore;
use charter_sys::secret_file::{read_secret, remove_secret, write_secret};
use charter_transport::Pairing;
use serde::{Deserialize, Serialize};

/// The warden's state directory (pairing, machine key, guardian-keys state).
pub const STATE_DIR: &str = "/var/lib/charter";

/// The display copy of the current presence token (`0600`).
pub const PRESENCE_TOKEN_FILE: &str = "presence-token.json";

/// An administrator's request for a presence token (`0600`, root-only dir).
pub const PRESENCE_REQUEST_FILE: &str = "presence-request.json";

/// How old a presence request may be when the daemon takes it. The helper
/// waits a few seconds for an answer; anything older was abandoned.
pub const PRESENCE_REQUEST_MAX_AGE_SECS: u64 = 120;

/// Where the daemon publishes [`GuardiansView`] (world-readable). Mirrors
/// `charter_ipc::real::GUARDIANS_VIEW_PATH`, which is behind the `real`
/// feature there.
pub const GUARDIANS_VIEW_PATH: &str = "/run/charter/guardians.json";

/// The separator between expected offline-unlock codes handed to the lock.
pub const UNLOCK_EXPECT_SEP: char = ',';

// ---------------------------------------------------------------------------
// The state and its adapter
// ---------------------------------------------------------------------------

/// The daemon's adapter for a paired ward, loaded ONCE per run (a ward that
/// paired before the section is migrated in place). The presence token is
/// also shown from [`PRESENCE_TOKEN_FILE`].
pub fn load_keys(dir: &Path, pairing: &Pairing, now: u64) -> GuardianKeys {
    let keys = GuardianKeys::load(Arc::new(FileGuardianStateStore::in_dir(dir)), pairing, now);
    keys.set_token_file(PresenceTokenStore::new(dir.join(PRESENCE_TOKEN_FILE)));
    keys
}

/// The state as a helper process may read it: never written, never migrated
/// on disk (the daemon is its one writer).
pub fn peek_keys(dir: &Path, pairing: &Pairing, now: u64) -> GuardianKeys {
    GuardianKeys::peek(Arc::new(FileGuardianStateStore::in_dir(dir)), pairing, now)
}

/// Forget the guardian-keys state, the presence token and any presence
/// request: what a local unpair or a fresh pin by an administrator does (rule
/// 11), so the new pairing never inherits a root, working key or token from
/// the old one. Absent files are fine.
pub fn clear_state(dir: &Path) -> std::io::Result<()> {
    for f in [
        GUARDIAN_STATE_FILE,
        PRESENCE_TOKEN_FILE,
        PRESENCE_REQUEST_FILE,
    ] {
        remove_secret(&dir.join(f))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Offline unlock (open question "Offline unlock with more than one guardian
// key"; review 2a, Info 7)
// ---------------------------------------------------------------------------

/// The expected 8-digit code for `challenge` from each key in the signer set
/// (deduplicated, sorted, so the order says nothing about the keys). Empty
/// when there are no keys: the lock then keeps its device-only escape. Each
/// conversation key is wiped as soon as its code is derived.
pub fn unlock_codes(machine_sk: &[u8; 32], signers: &[PubKey], challenge: &str) -> Vec<String> {
    let mut codes: Vec<String> = signers
        .iter()
        .filter_map(|k| {
            charter_transport::nip44::conversation_key_zeroizing(machine_sk, k.as_bytes()).ok()
        })
        .map(|conv| charter_crypto::unlock::unlock_code(&conv, challenge))
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

// ---------------------------------------------------------------------------
// Presence confirmation
// ---------------------------------------------------------------------------

/// Percent-encode everything but RFC 3986 unreserved characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The presence code to scan (open question "Presence code encoding
/// (build-defined)"):
/// `charter://presence?m=<machine>&t=<token>&d=<d>&s=<statement>`, `d`
/// percent-encoded, `s` the pending statement's event id (64 hex). The
/// confirming install may never have seen the statement, and the confirmation
/// must name its event id.
pub fn presence_payload(machine: &PubKey, token: &str, d: &str, statement: &str) -> String {
    format!(
        "charter://presence?m={}&t={token}&d={}&s={statement}",
        machine.to_hex(),
        percent_encode(d)
    )
}

/// The short form to type: the token as four groups of 8.
pub fn presence_short_form(token: &str) -> String {
    token
        .as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// An administrator's request, written by `charter-presence` (root) for the
/// daemon to serve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceRequest {
    /// The pending statement's `d` the token is bound to.
    pub d: String,
    pub requested_at: u64,
}

fn request_path(dir: &Path) -> PathBuf {
    dir.join(PRESENCE_REQUEST_FILE)
}

/// Ask the daemon for a presence token bound to `d`. The caller is the
/// privilege check: only root can write into the state directory, and the
/// file is `0600`.
pub fn request_presence(dir: &Path, d: &str, now: u64) -> std::io::Result<()> {
    let body = serde_json::to_vec(&PresenceRequest {
        d: d.to_owned(),
        requested_at: now,
    })
    .map_err(std::io::Error::other)?;
    write_secret(&request_path(dir), &body)
}

/// Whether a request is still waiting for the daemon.
pub fn presence_request_pending(dir: &Path) -> bool {
    request_path(dir).exists()
}

/// Withdraw a request the daemon never took (the helper gave up waiting).
pub fn withdraw_presence_request(dir: &Path) -> std::io::Result<()> {
    remove_secret(&request_path(dir))
}

/// What serving a request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceServed {
    Minted(MintedToken),
    /// Refused, with a reason for the log and the helper.
    Refused(String),
}

/// Daemon side: take a waiting request, if any, and mint a token for it. The
/// token is recorded in the state and shown from [`PRESENCE_TOKEN_FILE`]
/// (see [`load_keys`]); the request file is removed last, so a helper that
/// sees it gone reads the outcome from the token file. `None`: nothing was
/// waiting.
pub fn serve_presence_request(
    dir: &Path,
    keys: &GuardianKeys,
    now: u64,
    fill: &mut dyn FnMut(&mut [u8]),
) -> Option<PresenceServed> {
    let path = request_path(dir);
    let text = match read_secret(&path) {
        Ok(Some(t)) => t,
        Ok(None) => return None,
        Err(e) => {
            let _ = remove_secret(&path);
            return Some(PresenceServed::Refused(format!("unreadable request: {e}")));
        }
    };
    let served = match serde_json::from_str::<PresenceRequest>(&text) {
        Err(_) => PresenceServed::Refused("malformed request".into()),
        Ok(r) if r.requested_at > now.saturating_add(5) => {
            PresenceServed::Refused("request dated in the future".into())
        }
        Ok(r) if now.saturating_sub(r.requested_at) > PRESENCE_REQUEST_MAX_AGE_SECS => {
            PresenceServed::Refused("request too old".into())
        }
        Ok(r) => match keys.mint_presence_token(Some(&r.d), now, fill) {
            Ok(t) => PresenceServed::Minted(t),
            Err(PresenceError::NotPending) => {
                PresenceServed::Refused("nothing with that d is pending".into())
            }
            Err(PresenceError::Unavailable) => {
                PresenceServed::Refused("the guardian-keys state is unavailable".into())
            }
            Err(e) => PresenceServed::Refused(format!("could not mint: {e:?}")),
        },
    };
    if let PresenceServed::Refused(_) = served {
        let _ = PresenceTokenStore::new(dir.join(PRESENCE_TOKEN_FILE)).clear();
    }
    let _ = remove_secret(&path);
    Some(served)
}

/// Helper side: the token the daemon minted for `request`, if it did.
pub fn minted_for(
    dir: &Path,
    request: &PresenceRequest,
    now: u64,
) -> Option<charter_sys::presence_token::PresenceToken> {
    PresenceTokenStore::new(dir.join(PRESENCE_TOKEN_FILE))
        .current(now)
        .filter(|t| {
            t.bound_to.as_deref() == Some(request.d.as_str()) && t.minted_at >= request.requested_at
        })
}

// ---------------------------------------------------------------------------
// npub (NIP-19) for display
// ---------------------------------------------------------------------------

const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn bech32_polymod(values: &[u8]) -> u32 {
    const GEN: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];
    let mut chk: u32 = 1;
    for v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(*v);
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

/// `key` as a NIP-19 `npub`.
pub fn npub(key: &PubKey) -> String {
    const HRP: &str = "npub";
    let mut data = Vec::with_capacity(52);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in key.as_bytes() {
        acc = (acc << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            data.push(((acc >> bits) & 31) as u8);
        }
    }
    if bits > 0 {
        data.push(((acc << (5 - bits)) & 31) as u8);
    }
    let mut values: Vec<u8> = HRP.bytes().map(|c| c >> 5).collect();
    values.push(0);
    values.extend(HRP.bytes().map(|c| c & 31));
    values.extend(&data);
    values.extend([0u8; 6]);
    let pm = bech32_polymod(&values) ^ 1;
    let mut out = String::from("npub1");
    for d in &data {
        out.push(BECH32_CHARSET[*d as usize] as char);
    }
    for i in 0..6 {
        out.push(BECH32_CHARSET[((pm >> (5 * (5 - i))) & 31) as usize] as char);
    }
    out
}

/// A short npub to show beside a label: `npub1` and 8 more characters, an
/// ellipsis, and the last 6.
pub fn short_npub(key: &PubKey) -> String {
    let n = npub(key);
    format!("{}…{}", &n[..13], &n[n.len() - 6..])
}

// ---------------------------------------------------------------------------
// The world-readable view (rule 5)
// ---------------------------------------------------------------------------

/// Guardian-chosen text, safe to print: no control or invisible format
/// characters (bidi overrides included), at most 64 bytes (review batch 2,
/// platforms L7).
fn clean_label(s: &str) -> String {
    charter_spine::clean_label(s)
}

/// Who the ward trusts and who set each rule it holds, for the CLI and the
/// tray. `None` when the state is unavailable.
pub fn build_view(keys: &GuardianKeys, now: u64) -> Option<GuardiansView> {
    let projected = keys.project(now)?;
    let short_hex = |h: &str| PubKey::from_hex(h).ok().map(|k| short_npub(&k));
    let roots = projected
        .roots
        .iter()
        .filter_map(|r| short_hex(&r.root))
        .collect();
    let rules = projected
        .clauses
        .iter()
        .filter_map(|c| {
            let root = short_hex(&c.root)?;
            let label = PubKey::from_hex(&c.signer)
                .ok()
                .and_then(|k| keys.signer_label(&k))
                .map(|l| clean_label(&l))
                .filter(|l| !l.is_empty());
            Some(RuleSetterView {
                subject: c.subject.clone(),
                kind: c.kind.clone(),
                root,
                label,
            })
        })
        .collect();
    let pending = keys
        .pending_statements(now)
        .into_iter()
        .map(|p| {
            let key = charter_guardians::split_pending_d(&p.d)
                .and_then(|(_, k)| short_hex(k))
                .unwrap_or_default();
            PendingStatementView {
                kind: pending_kind(p.kind).to_owned(),
                d: p.d,
                key,
                takes_effect_at: p.takes_effect_at,
                expires_at: p.expires_at,
            }
        })
        .collect();
    Some(GuardiansView {
        roots,
        rules,
        pending,
    })
}

/// The STATUS wire name of a pending statement's kind.
pub fn pending_kind(k: charter_guardians::PendingType) -> &'static str {
    match k {
        charter_guardians::PendingType::Coguardian => "coguardian",
        charter_guardians::PendingType::Removal => "removal",
        charter_guardians::PendingType::Move => "move",
    }
}

/// Write the view world-readable (`0644`, temp file and rename), as the
/// per-child state files are: a ward can always see who governs them.
pub fn publish_view(path: &Path, view: &GuardiansView) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_vec(view).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, path)
}

/// Run the timers (a removal's notice ending, pending statements expiring)
/// outside a broker round, so STATUS and the view are current even while the
/// relays are down. The broker advances them too, before every judgement.
pub fn advance(keys: &GuardianKeys, now: u64) -> Vec<PubKey> {
    GuardianStatements::advance(keys, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npub_matches_the_nip19_example() {
        let k =
            PubKey::from_hex("3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d")
                .unwrap();
        assert_eq!(
            npub(&k),
            "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
        );
        assert_eq!(short_npub(&k), "npub180cvv07t…yjh6w6");
    }

    #[test]
    fn the_presence_code_mirrors_the_pair_invite() {
        let m = PubKey::from_bytes([0xab; 32]);
        let d = format!("kintrinsic/guardian/{}", "cd".repeat(32));
        assert_eq!(
            presence_payload(&m, &"ef".repeat(16), &d, &"12".repeat(32)),
            format!(
                "charter://presence?m={}&t={}&d=kintrinsic%2Fguardian%2F{}&s={}",
                "ab".repeat(32),
                "ef".repeat(16),
                "cd".repeat(32),
                "12".repeat(32)
            )
        );
        assert_eq!(
            presence_short_form("0123456789abcdef0123456789abcdef"),
            "01234567 89abcdef 01234567 89abcdef"
        );
    }

    #[test]
    fn labels_lose_control_characters_and_are_capped() {
        assert_eq!(clean_label("Mum\u{1b}[2J's\nphone"), "Mum[2J'sphone");
        assert_eq!(clean_label(&"x".repeat(100)).len(), 64);
        assert_eq!(clean_label("\u{202E}Mum\u{2066}"), "Mum");
    }

    #[test]
    fn an_unlock_code_per_signer_and_none_without_signers() {
        let sk = [7u8; 32];
        let a = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[1u8; 32]).unwrap());
        let b = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[2u8; 32]).unwrap());
        let codes = unlock_codes(&sk, &[a, b, a], "K7QN");
        assert_eq!(codes.len(), 2);
        let conv = charter_transport::nip44::conversation_key_zeroizing(&sk, a.as_bytes()).unwrap();
        assert!(codes.contains(&charter_crypto::unlock::unlock_code(&conv, "K7QN")));
        assert!(codes.iter().all(|c| !c.contains(UNLOCK_EXPECT_SEP)));
        assert!(unlock_codes(&sk, &[], "K7QN").is_empty());
    }

    #[test]
    fn clearing_the_state_removes_every_file_and_tolerates_none() {
        let dir = std::env::temp_dir().join(format!("charter-gk-clear-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            GUARDIAN_STATE_FILE,
            PRESENCE_TOKEN_FILE,
            PRESENCE_REQUEST_FILE,
        ] {
            write_secret(&dir.join(f), b"{}").unwrap();
        }
        clear_state(&dir).unwrap();
        for f in [
            GUARDIAN_STATE_FILE,
            PRESENCE_TOKEN_FILE,
            PRESENCE_REQUEST_FILE,
        ] {
            assert!(!dir.join(f).exists(), "{f}");
        }
        clear_state(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
