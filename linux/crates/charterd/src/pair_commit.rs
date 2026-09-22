//! Turn an accepted offer into a pinned pairing on disk — the same end state
//! `charter-pair` reaches from a pasted `bunker://` link, so the scan path and
//! the typed path converge and the daemon beneath them is unchanged.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use charter_primitives::PubKey;
use charter_sys::relay::RelayUrl;

use crate::device_limits::{load_child_configs, purge_subject_store, set_child_subject, CHILD_CLAUSE_STORE_BASE};
use crate::pairing_setup::build_pairing_json;

/// Where a pin lands. Injected rather than hardcoded so the whole commit is
/// testable in a temp dir.
pub struct PinPaths {
    pub pairing: String,
    pub limits_dir: String,
    pub token: String,
    /// The root a re-pair purges an orphaned OLD subject's clauses under —
    /// `<children_base>/children/<subject_hex>/clauses`, the same tree
    /// `RealChildClauseStore` writes (`/var/lib/charter` in production).
    pub children_base: String,
}

impl PinPaths {
    /// The production locations.
    pub fn production() -> Self {
        PinPaths {
            pairing: "/var/lib/charter/pairing.json".into(),
            limits_dir: "/etc/charter/limits.d".into(),
            token: "/var/lib/charter/pair-token.json".into(),
            children_base: CHILD_CLAUSE_STORE_BASE.into(),
        }
    }
}

/// Write the pin, bind the subject, spend the token.
///
/// The subject is minted **locally**: Kintrinsic has no dependant pubkey to
/// offer (`Child.dependantPubkey` is null and never assigned), and the broker
/// routes a subject-less clause to the pairing's sole subject. A locally drawn
/// subject is therefore both correct and sufficient.
///
/// Order matters: bind the child FIRST. A pairing with no bound subject leaves
/// the guardian's clauses inert while the UI cheerfully claims success — the
/// exact trap `charter-pair` documents at its step 6. But binding first means a
/// failed pin write must not leave the child bound to a subject no guardian
/// holds either — so a failed write ROLLS BACK the bind (best effort) before
/// returning the error, making both orderings safe.
pub fn commit_pin(
    paths: &PinPaths,
    guardian: PubKey,
    relays: &[RelayUrl],
    machine: PubKey,
    subject_random: [u8; 32],
    now: u64,
) -> Result<(), String> {
    let children = load_child_configs(&paths.limits_dir);
    let child = match children.as_slice() {
        [] => return Err("no child is set up on this computer yet".into()),
        [(user, _)] => user.clone(),
        _ => return Err("more than one child on this computer — pair from the app".into()),
    };
    let previous_subject = children
        .iter()
        .find(|(u, _)| u == &child)
        .and_then(|(_, c)| c.subject.clone());

    let subject_hex: String = subject_random.iter().map(|b| format!("{b:02x}")).collect();
    let subject = PubKey::from_hex(&subject_hex).map_err(|_| "bad subject".to_string())?;

    // Rebuild the canonical `bunker://` form so the pin goes through exactly
    // the same validator the pasted link does — one grammar, one code path.
    let relay_params = relays
        .iter()
        .map(|r| format!("relay={r}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("bunker://{}?{relay_params}&kind=charter", guardian.to_hex());
    let json = build_pairing_json(&uri, machine, subject, now)
        .map_err(|_| "could not build the pairing from that offer".to_string())?;

    // Whether this is a genuine re-pair (there was already a pinned guardian)
    // — decided BEFORE the write below, since that write is what the rest of
    // this function is about to attempt.
    let pairing_existed = Path::new(&paths.pairing).exists();

    set_child_subject(&paths.limits_dir, &child, Some(&subject_hex))
        .map_err(|e| format!("could not link {child}: {e}"))?;

    let write_result: Result<(), String> = (|| {
        if let Some(dir) = Path::new(&paths.pairing).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = format!("{}.tmp", paths.pairing);
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        // 0644 — the pin is PUBLIC data (guardian pubkey + relays), and the
        // unprivileged status surfaces read the guardian's short fingerprint
        // here.
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &paths.pairing).map_err(|e| e.to_string())
    })();
    if let Err(e) = write_result {
        if let Err(re) = set_child_subject(&paths.limits_dir, &child, previous_subject.as_deref())
        {
            eprintln!(
                "charterd: could not roll back {child}'s subject link after a failed pin \
                 write ({e}): {re}"
            );
        }
        return Err(e);
    }

    crate::pair_token::consume(&paths.token).map_err(|e| e.to_string())?;

    // The pin landed. On a genuine re-pair to a NEW subject, the OLD
    // subject's cached clauses are now orphaned — purge them, the same
    // directory a guardian RELEASE purges. Best effort: the pairing already
    // succeeded, so a purge failure must not undo it.
    if pairing_existed {
        if let Some(prev) = &previous_subject {
            if prev.to_lowercase() != subject_hex {
                if let Err(e) = purge_subject_store(&paths.children_base, prev) {
                    eprintln!(
                        "charterd: could not clear the old subject's cached clauses on \
                         re-pair: {e}"
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (PinPaths, String) {
        let d = std::env::temp_dir().join(format!("charter-pin-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("limits.d")).unwrap();
        let child = d.join("limits.d/axel.json");
        std::fs::write(
            &child,
            r#"{"limits":{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}"#,
        )
        .unwrap();
        let paths = PinPaths {
            pairing: d.join("pairing.json").to_string_lossy().into_owned(),
            limits_dir: d.join("limits.d").to_string_lossy().into_owned(),
            token: d.join("pair-token.json").to_string_lossy().into_owned(),
            children_base: d.to_string_lossy().into_owned(),
        };
        (paths, child.to_string_lossy().into_owned())
    }

    fn clauses_dir(paths: &PinPaths, subject_hex: &str) -> std::path::PathBuf {
        Path::new(&paths.children_base)
            .join("children")
            .join(subject_hex)
            .join("clauses")
    }

    fn relays() -> Vec<RelayUrl> {
        vec!["wss://relay.trotters.cc".to_string()]
    }

    #[test]
    fn writes_the_pin_binds_the_subject_and_spends_the_token() {
        let (paths, child) = fixture("ok");
        crate::pair_token::mint(&paths.token, 10, [0x22; 16]).unwrap();

        commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap();

        let pinned = std::fs::read_to_string(&paths.pairing).unwrap();
        assert!(
            pinned.contains(&"aa".repeat(32)),
            "guardian pinned: {pinned}"
        );

        // The subject is bound to the sole child, so clauses RESOLVE rather
        // than merely cache — the difference between working and inert.
        let kid = std::fs::read_to_string(&child).unwrap();
        assert!(kid.contains(&"cc".repeat(32)), "subject bound: {kid}");

        // Token spent — a replayed offer finds nothing.
        assert!(crate::pair_token::current(&paths.token, 100).is_none());
    }

    #[test]
    fn the_pin_is_world_readable_but_the_token_is_gone() {
        let (paths, _) = fixture("perms");
        crate::pair_token::mint(&paths.token, 10, [0x22; 16]).unwrap();
        commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap();
        let mode = std::fs::metadata(&paths.pairing)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644);
        assert!(!Path::new(&paths.token).exists());
    }

    #[test]
    fn refuses_when_there_is_no_child_to_bind() {
        let (paths, child) = fixture("nokid");
        std::fs::remove_file(&child).unwrap();
        let err = commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap_err();
        assert!(err.contains("child"), "{err}");
        // Nothing half-written: no pin claiming a success that never happened.
        assert!(!Path::new(&paths.pairing).exists());
    }

    #[test]
    fn refuses_an_offer_carrying_no_secure_relay() {
        let (paths, _) = fixture("norelay");
        let err = commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &[],
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap_err();
        assert!(err.contains("pairing"), "{err}");
        assert!(!Path::new(&paths.pairing).exists());
    }

    #[test]
    fn a_failed_pin_write_rolls_back_the_subject_bind() {
        let (paths, child) = fixture("failwrite");
        // Make the destination an existing directory so the final rename
        // fails (renaming a file onto a directory always fails) — the child
        // config has no `subject` yet, so a correct rollback restores that.
        std::fs::create_dir_all(&paths.pairing).unwrap();

        let err = commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap_err();
        assert!(!err.is_empty());

        let cfg =
            crate::device_limits::parse_child_config(&std::fs::read_to_string(&child).unwrap())
                .unwrap();
        assert_eq!(
            cfg.subject, None,
            "the bind-then-write failed, so the child's subject must be exactly what it was \
             before the attempt"
        );
    }

    #[test]
    fn a_repair_to_a_new_subject_purges_the_old_subjects_clauses() {
        let (paths, child) = fixture("repair-new");
        let old_hex = "a".repeat(64);
        // The child is already bound to an OLD subject, as if previously
        // paired.
        std::fs::write(
            &child,
            format!(
                r#"{{"subject":"{old_hex}","limits":{{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}}}"#
            ),
        )
        .unwrap();
        // A pinned pairing already exists — content is irrelevant, only
        // existence decides whether this is a re-pair.
        std::fs::write(&paths.pairing, "{}").unwrap();
        // The old subject's cached clauses — what a re-pair must purge.
        let old_dir = clauses_dir(&paths, &old_hex);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("1.json"), "{}").unwrap();

        commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32], // hex "cc"*32 — a different subject than old_hex
            100,
        )
        .unwrap();

        assert!(!old_dir.exists(), "the old subject's clauses are purged");
        let new_hex = "cc".repeat(32);
        let cfg =
            crate::device_limits::parse_child_config(&std::fs::read_to_string(&child).unwrap())
                .unwrap();
        assert_eq!(cfg.subject.as_deref(), Some(new_hex.as_str()));
    }

    #[test]
    fn a_repair_to_the_same_subject_purges_nothing() {
        let (paths, child) = fixture("repair-same");
        let hex = "cc".repeat(32); // matches subject_random [0xCC; 32] below
        std::fs::write(
            &child,
            format!(
                r#"{{"subject":"{hex}","limits":{{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(&paths.pairing, "{}").unwrap();
        let dir = clauses_dir(&paths, &hex);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.json"), "{}").unwrap();

        commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays(),
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap();

        assert!(dir.exists(), "same-subject re-pair purges nothing");
    }
}
