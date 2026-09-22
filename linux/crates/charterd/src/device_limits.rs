//! Device-only limits: standalone parental controls with **no guardian app**.
//!
//! A root-owned `/etc/charter/limits.json` lets a parent set an allowed-hours
//! window + a daily time cap directly on the box. `charterd` turns it into the
//! same `GrantSchedule` + `GrantBudget` the enforcer already consumes and writes
//! them into the clause store, so the existing freeze/lock loop enforces them —
//! no signing, no relay, no phone.
//!
//! Security: only **root** can write `limits.json` (the managed child cannot), so
//! this does not hand the child any authority — it is the parent acting locally
//! instead of remotely. Brokered effects (install/exec) still require a verified
//! guardian grant; device-only governs *time* only.
//!
//! The MODEL + conversion (`DeviceLimits`, `ChildConfig`, the form/passwd
//! parsers) live in `charter_spine::local_limits` — pure, shared, re-exported
//! here so existing callers keep their paths. THIS module is the Linux file-IO
//! half: loading/saving the `/etc/charter` documents. The loop wiring is in
//! [`crate::runtime`].

pub use charter_spine::local_limits::*;

use charter_sys::SysResult;

/// Read + parse `limits.json`, returning the limits and the file mtime (used as
/// the clause `issued_at` so an edit supersedes and an untouched file is a
/// no-op). `Ok(None)` when the file is absent (device-only not configured).
pub fn load_device_limits(path: &str) -> SysResult<Option<(DeviceLimits, u64)>> {
    use std::time::UNIX_EPOCH;
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(charter_sys::SysError::Io(e.to_string())),
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(1);
    let text =
        std::fs::read_to_string(path).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    let limits: DeviceLimits =
        serde_json::from_str(&text).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    limits.validate().map_err(charter_sys::SysError::Io)?;
    Ok(Some((limits, mtime.max(1))))
}

/// Atomically write `limits` as pretty JSON (the settings UI / CLI saves here;
/// `charterd` picks it up within a tick). Root-only by file permissions.
pub fn save_device_limits(path: &str, limits: &DeviceLimits) -> SysResult<()> {
    limits.validate().map_err(charter_sys::SysError::Io)?;
    let json = serde_json::to_string_pretty(limits)
        .map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| charter_sys::SysError::Io(e.to_string()))
}

/// Load every `<dir>/<username>.json` as a [`ChildConfig`] (filename stem ==
/// username), accepting both the structured and legacy flat shapes. Skips files
/// that don't parse/validate. Sorted by username. The multi-child source for the
/// Signet-first resolver.
pub fn load_child_configs(dir: &str) -> Vec<(String, ChildConfig)> {
    let rd = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(user) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(cfg) = parse_child_config(&text) {
                out.push((user.to_string(), cfg));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Set (or clear, with `None`) the guardian subject binding for `username`,
/// preserving any existing device-only limits. Loads the current config (flat or
/// structured), updates the binding, and writes the structured form atomically.
/// The admin-only pairing step (`charter-setup`) calls this. Rejects an
/// ill-formed subject hex.
pub fn set_child_subject(dir: &str, username: &str, subject_hex: Option<&str>) -> SysResult<()> {
    reject_bad_username(username)?;
    if let Some(s) = subject_hex {
        if !valid_subject_hex(s) {
            return Err(charter_sys::SysError::Io(
                "subject must be 64 hex chars".into(),
            ));
        }
    }
    // 02b-G8: one subject, one child. Two children bound to the same guardian
    // subject share a clause set, a usage-pool slot and a STATUS identity —
    // the guardian's consolidator reads them as one device reporting twice, so
    // one sibling's usage overwrites the other's, and a `time.extend` approved
    // for one lands on whichever uid the loop happens to find first. Refuse at
    // the point the state would be created; `child_policy::drop_duplicate_subjects`
    // is the load-time backstop for a file that got there another way.
    if let Some(s) = subject_hex {
        let wanted = s.to_lowercase();
        if let Some(other) = load_child_configs(dir)
            .into_iter()
            .find(|(user, cfg)| user != username && cfg.subject.as_deref() == Some(wanted.as_str()))
        {
            return Err(charter_sys::SysError::Io(format!(
                "that guardian subject is already bound to {} — each child needs their \
                 own; unbind {} first, or pair this child separately",
                other.0, other.0
            )));
        }
    }
    let path = child_limits_path(dir, username);
    let mut cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| parse_child_config(&t))
        .unwrap_or_default();
    cfg.subject = subject_hex.map(|s| s.to_lowercase());
    save_child_config(&path, &cfg)
}

/// Set the device-only **limits** for `username`, **preserving** any existing
/// guardian `subject` binding. Mirrors [`set_child_subject`] (load-merge-write of
/// the structured form): the settings GUI must never clobber a paired child's
/// binding and silently revert them from guardian control to device-only. Loads
/// the current config (flat or structured), replaces only `limits`, and writes
/// the structured form atomically.
pub fn set_child_limits(dir: &str, username: &str, limits: &DeviceLimits) -> SysResult<()> {
    reject_bad_username(username)?;
    limits.validate().map_err(charter_sys::SysError::Io)?;
    let path = child_limits_path(dir, username);
    let mut cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| parse_child_config(&t))
        .unwrap_or_default();
    cfg.limits = Some(limits.clone());
    save_child_config(&path, &cfg)
}

/// Set (or clear, with `None`) a child's device-only learning apps,
/// preserving their limits and guardian binding. The read-modify-write
/// mirrors [`set_child_limits`].
pub fn set_child_learning(
    dir: &str,
    username: &str,
    learning: Option<&charter_proto::GrantLearning>,
) -> SysResult<()> {
    reject_bad_username(username)?;
    let path = child_limits_path(dir, username);
    let mut cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| parse_child_config(&t))
        .unwrap_or_default();
    cfg.learning = learning.cloned();
    save_child_config(&path, &cfg)
}

/// Atomically write a [`ChildConfig`] as pretty JSON (root-only by file perms).
pub fn save_child_config(path: &str, cfg: &ChildConfig) -> SysResult<()> {
    if let Some(l) = &cfg.limits {
        l.validate().map_err(charter_sys::SysError::Io)?;
    }
    let json =
        serde_json::to_string_pretty(cfg).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, json.as_bytes()).map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| charter_sys::SysError::Io(e.to_string()))
}

/// Load every `<dir>/<username>.json` as a child's limits (the filename stem is
/// the username). Skips files without device-only limits. Sorted by username
/// for deterministic order. The settings-GUI view (device-only children).
pub fn load_child_limits_dir(dir: &str) -> Vec<(String, DeviceLimits)> {
    load_child_configs(dir)
        .into_iter()
        .filter_map(|(user, cfg)| cfg.limits.map(|l| (user, l)))
        .collect()
}

/// Whether `username` is a safe local-account name to build a state-file path
/// from — a POSIX-style login name (`[a-z_][a-z0-9_-]*`, max 32). This rejects
/// `/`, `.`/`..`, and NUL, so a name can never escape the limits directory.
///
/// Security-critical: the `charter-settings` helper runs as **root** under
/// `pkexec`, and its `--set <user>` argument flows straight into
/// [`child_limits_path`] → an atomic write. Without this guard a caller (any
/// script, or a future/compromised console UI) could pass e.g.
/// `--set ../../../../etc/cron.d/x` and have root create directories and write
/// an attacker-chosen file anywhere on the box. The three setters below reject
/// an invalid name before ever building a path from it — mirroring the
/// `id "$MANAGED_USER"` existence check the older `charter-setup` script does.
pub fn valid_username(username: &str) -> bool {
    let mut bytes = username.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_lowercase() || first == b'_' => {}
        _ => return false,
    }
    username.len() <= 32
        && username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn reject_bad_username(username: &str) -> SysResult<()> {
    if valid_username(username) {
        Ok(())
    } else {
        Err(charter_sys::SysError::Io(format!(
            "invalid username: {username:?} (expected a POSIX login name)"
        )))
    }
}

/// The per-child limits path for `username` under `dir` (the settings UI writes
/// here). Callers that accept an untrusted `username` MUST gate it through
/// [`valid_username`] first — see the three `set_child_*` setters.
pub fn child_limits_path(dir: &str, username: &str) -> String {
    format!("{dir}/{username}.json")
}

/// The root a guardian RELEASE purges a child's cached clauses under
/// (`RealChildClauseStore` in `charter_sys::persistence`, `/var/lib/charter` in
/// production). Exposed here so callers that can't always build under the
/// `real` feature (the `charter-pair` binaries, `pair_commit`) still purge the
/// same directory on a re-pair.
pub const CHILD_CLAUSE_STORE_BASE: &str = "/var/lib/charter";

/// Remove `subject_hex`'s per-child clause store under `base` —
/// `<base>/children/<subject_hex>/clauses`, using the SAME subject rule
/// `RealChildClauseStore` now enforces, so a crafted subject can never escape
/// `base`. `charter-pair`/`pair_commit` call this on a re-pair to a NEW
/// subject: the old subject's clauses are otherwise orphaned — cached but
/// unreachable — which is the same state a guardian RELEASE clears via that
/// trait method. This mirrors its logic directly rather than depending on it,
/// since `charter_sys`'s real filesystem impls are gated behind the `real`
/// feature and this module builds under `mock` too (`pair_commit`'s tests run
/// there). Missing already ⇒ `Ok(())` (nothing to purge is not a failure).
///
/// # The subject is REJECTED, never sanitised (02-B8)
///
/// This used to FILTER: strip every non-hex character, lowercase the rest, and
/// use whatever survived. That is the dangerous shape — it always produces
/// *some* path, so `"../../etc"` and `"AA…AA"` and a 12-character fragment all
/// name a directory somebody might have, and the one it names is not the one
/// the caller asked about. `RealChildClauseStore::safe` now demands exactly 64
/// lowercase hex characters and refuses anything else; a purge that quietly
/// accepted more would be the surviving half of the same hole, deleting a
/// directory on the strength of a string that is not a subject.
pub fn purge_subject_store(base: &str, subject_hex: &str) -> SysResult<()> {
    let conforms = subject_hex.len() == 64
        && subject_hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !conforms {
        // The subject itself is never echoed: it is caller-controlled and
        // unbounded, exactly as `bad_subject` reasons in charter-sys.
        eprintln!(
            "charter: refusing to purge a clause store for a subject that is not 64 \
             lowercase hex characters (len {}) — nothing was touched",
            subject_hex.len()
        );
        return Ok(());
    }
    let dir = std::path::Path::new(base)
        .join("children")
        .join(subject_hex)
        .join("clauses");
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(charter_sys::SysError::Io(e.to_string())),
    }
}

/// The system timezone, resolved the same chain `charter-setup` uses (so the
/// settings UI and the setup wizard never disagree about what "the
/// timezone" is), and validated the same way. This used to read only
/// `/etc/timezone` — absent on plenty of systemd hosts where the timezone
/// lives solely in the `/etc/localtime` symlink — and fell back to a
/// hardcoded `"UTC"` on any failure, silently shifting a child's wake/bedtime
/// hours with no error anywhere.
///
/// 1) `timedatectl`'s own idea of it (works everywhere systemd runs, not
///    just Debian).
/// 2) the `/etc/localtime` symlink target, resolved to an absolute path,
///    which is how the TZ actually takes effect regardless of what any text
///    file says.
/// 3) `/etc/timezone` (Debian/Ubuntu's own record) as a last resort.
///
/// `Err` when none of those resolve to a validated name. Callers must not
/// paper over that with a hardcoded `"UTC"` — see `charter-settings.rs`.
pub fn detect_tz() -> SysResult<String> {
    if let Some(tz) = std::process::Command::new("timedatectl")
        .args(["show", "-p", "Timezone", "--value"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| tz_is_valid(s))
    {
        return Ok(tz);
    }
    if let Some(tz) = std::fs::canonicalize("/etc/localtime")
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .and_then(|s| s.split("/zoneinfo/").nth(1).map(|s| s.to_string()))
        .filter(|s| tz_is_valid(s))
    {
        return Ok(tz);
    }
    if let Some(tz) = std::fs::read_to_string("/etc/timezone")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| tz_is_valid(s))
    {
        return Ok(tz);
    }
    Err(charter_sys::SysError::Unsupported(
        "could not determine this system's timezone (checked timedatectl, \
         /etc/localtime, /etc/timezone)"
            .to_string(),
    ))
}

/// The pure half of zone-name validation: non-empty `/`-separated segments
/// of alphanumerics/`_`/`+`/`-` only (`^[A-Za-z0-9_+-]+(/[A-Za-z0-9_+-]+)*$`
/// — the same shape `charter-setup` requires). Split out from `tz_is_valid`
/// so it can be unit-tested without touching the filesystem.
fn tz_name_shape_ok(tz: &str) -> bool {
    !tz.is_empty()
        && tz.split('/').all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
        })
}

/// A validated IANA zone name: `tz_name_shape_ok`, AND a real file under
/// `/usr/share/zoneinfo` for it. `timedatectl` prints the literal string
/// `"n/a"` when it doesn't know the timezone; that passes the shape check
/// but `/usr/share/zoneinfo/n/a` doesn't exist, so the existence check
/// correctly rejects it rather than treating it as a zone.
fn tz_is_valid(tz: &str) -> bool {
    tz_name_shape_ok(tz) && std::path::Path::new("/usr/share/zoneinfo").join(tz).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX64: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn sample() -> DeviceLimits {
        DeviceLimits {
            tz: "Europe/London".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 120,
            weekend: Some(DayWindow {
                wake: "08:00".into(),
                bedtime: "21:00".into(),
            }),
        }
    }

    #[test]
    fn purge_subject_store_removes_only_the_named_subjects_clauses_dir() {
        let d = std::env::temp_dir().join("charter-purge-subject-test");
        let _ = std::fs::remove_dir_all(&d);
        let old = format!("{d}/children/{HEX64}/clauses", d = d.to_string_lossy());
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(format!("{old}/1.json"), "{}").unwrap();
        let other_hex = "b".repeat(64);
        let other = format!("{d}/children/{other_hex}/clauses", d = d.to_string_lossy());
        std::fs::create_dir_all(&other).unwrap();

        purge_subject_store(&d.to_string_lossy(), HEX64).unwrap();

        assert!(!std::path::Path::new(&old).exists(), "purged");
        assert!(std::path::Path::new(&other).exists(), "untouched");
        // Missing already: not an error.
        assert!(purge_subject_store(&d.to_string_lossy(), HEX64).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 02b-G8: the write path is where this state would be created, so it is
    /// the first place it is refused.
    #[test]
    fn set_child_subject_refuses_a_subject_another_child_already_holds() {
        let dir = std::env::temp_dir().join("charter-dup-subject-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();

        set_child_subject(d, "alice", Some(HEX64)).unwrap();
        let err = set_child_subject(d, "bob", Some(HEX64))
            .expect_err("two children must not share one guardian subject");
        assert!(
            format!("{err}").contains("alice"),
            "the refusal names who already holds it: {err}"
        );
        // Bob is untouched — not half-bound.
        let bob = load_child_configs(d).into_iter().find(|(u, _)| u == "bob");
        assert!(bob.is_none() || bob.unwrap().1.subject.is_none());

        // Re-binding the SAME child to the same subject is a no-op, not a clash.
        set_child_subject(d, "alice", Some(HEX64)).unwrap();
        // Case is normalised before the comparison, so uppercase clashes too.
        assert!(set_child_subject(d, "bob", Some(&HEX64.to_uppercase())).is_err());
        // Once alice lets go, bob may have it.
        set_child_subject(d, "alice", None).unwrap();
        set_child_subject(d, "bob", Some(HEX64)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 02-B8: a subject that is not exactly 64 lowercase hex must be REFUSED,
    /// not filtered down into some other child's directory. The uppercase case
    /// is the sharp one — the old sanitiser lowercased it, so purging
    /// `"AAAA…"` silently deleted `"aaaa…"`'s clauses.
    #[test]
    fn purge_subject_store_refuses_a_subject_that_is_not_64_lowercase_hex() {
        let d = std::env::temp_dir().join("charter-purge-subject-reject-test");
        let _ = std::fs::remove_dir_all(&d);
        let base = d.to_string_lossy().into_owned();
        let live = format!("{base}/children/{HEX64}/clauses");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(format!("{live}/1.json"), "{}").unwrap();

        for bad in [
            HEX64.to_uppercase(),    // lowercased into the live subject
            format!("{HEX64}-"),     // hex plus a stray character
            "../../etc".to_string(), // traversal, filtered down to "ec"
            "aabb".to_string(),      // a fragment
            String::new(),
        ] {
            assert!(
                purge_subject_store(&base, &bad).is_ok(),
                "a refusal is not a storage failure"
            );
            assert!(
                std::path::Path::new(&live).exists(),
                "nothing may be deleted on the strength of {bad:?}"
            );
        }
        // The real subject still purges.
        purge_subject_store(&base, HEX64).unwrap();
        assert!(!std::path::Path::new(&live).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn valid_username_accepts_logins_and_rejects_traversal() {
        for ok in ["alice", "bob_2", "a", "child-1", "_svc", "user123"] {
            assert!(valid_username(ok), "should accept {ok:?}");
        }
        for bad in [
            "",
            "../etc/passwd",
            "..",
            "a/b",
            "alice.json",
            "Alice",                               // uppercase not a POSIX login start
            "1alice",                              // must not start with a digit
            "a b",                                 // space
            "a\0b",                                // NUL
            "toolongtoolongtoolongtoolongtoolong", // > 32
        ] {
            assert!(!valid_username(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn setters_reject_a_traversal_username_without_writing() {
        let dir = std::env::temp_dir().join(format!("charter-uv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        // A path-traversal name must be refused by every root-run setter, and
        // nothing may be written outside the limits dir.
        let evil = "../charter-escape";
        assert!(set_child_limits(d, evil, &sample()).is_err());
        assert!(set_child_subject(d, evil, Some(HEX64)).is_err());
        assert!(set_child_learning(d, evil, None).is_err());
        assert!(
            !std::path::Path::new(&format!("{d}/../charter-escape.json")).exists(),
            "no file may be written outside the limits directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_child_limits_dir_reads_per_child_files() {
        let dir = std::env::temp_dir().join(format!("charter-ld-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        save_device_limits(&child_limits_path(d, "alice"), &sample()).unwrap();
        let mut younger = sample();
        younger.daily_minutes = 30;
        save_device_limits(&child_limits_path(d, "bob"), &younger).unwrap();
        std::fs::write(format!("{d}/notjson.txt"), "ignore me").unwrap();

        let loaded = load_child_limits_dir(d);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].0, "alice"); // sorted
        assert_eq!(loaded[1].0, "bob");
        assert_eq!(loaded[1].1.daily_minutes, 30);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("charter-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("limits.json");
        let p = path.to_str().unwrap();
        save_device_limits(p, &sample()).unwrap();
        let (back, _mtime) = load_device_limits(p).unwrap().unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn set_child_subject_converts_flat_to_structured_and_keeps_limits() {
        let dir = std::env::temp_dir().join(format!("charter-cc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();
        // Start with a legacy flat device-only file.
        save_device_limits(&child_limits_path(d, "alice"), &sample()).unwrap();
        // Setup binds Alice to a guardian subject.
        set_child_subject(d, "alice", Some(HEX64)).unwrap();
        let cfg = load_child_configs(d)
            .into_iter()
            .find(|(u, _)| u == "alice")
            .map(|(_, c)| c)
            .unwrap();
        assert_eq!(cfg.subject.as_deref(), Some(HEX64));
        assert_eq!(
            cfg.limits.as_ref().unwrap().daily_minutes,
            120,
            "the device-only fallback survives binding"
        );
        // Clearing the binding leaves the limits.
        set_child_subject(d, "alice", None).unwrap();
        let cfg2 =
            parse_child_config(&std::fs::read_to_string(child_limits_path(d, "alice")).unwrap())
                .unwrap();
        assert_eq!(cfg2.subject, None);
        assert!(cfg2.limits.is_some());
    }

    #[test]
    fn set_child_limits_preserves_guardian_subject_binding() {
        // The settings GUI saving device-only limits for a child who is ALSO bound
        // to a guardian must keep the binding — otherwise the next tick parses a
        // flat doc (subject=None) and silently reverts the child from guardian
        // control to device-only (a guardian-authority regression).
        let dir = std::env::temp_dir().join(format!("charter-scl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();
        // Bound child (subject + limits), as charter-pair leaves it.
        save_child_config(
            &child_limits_path(d, "alice"),
            &ChildConfig {
                subject: Some(HEX64.into()),
                limits: Some(sample()),
                learning: None,
            },
        )
        .unwrap();
        // Parent tweaks the daily cap in "Kintrinsic Screen Time".
        let mut tweaked = sample();
        tweaked.daily_minutes = 45;
        set_child_limits(d, "alice", &tweaked).unwrap();

        let cfg =
            parse_child_config(&std::fs::read_to_string(child_limits_path(d, "alice")).unwrap())
                .unwrap();
        assert_eq!(
            cfg.subject.as_deref(),
            Some(HEX64),
            "the guardian binding must survive a device-only limits edit"
        );
        assert_eq!(
            cfg.limits.as_ref().unwrap().daily_minutes,
            45,
            "the new device-only limits are written"
        );
    }

    #[test]
    fn set_child_learning_preserves_limits_and_binding() {
        let dir = std::env::temp_dir().join(format!("charter-scl3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();
        save_child_config(
            &child_limits_path(d, "alice"),
            &ChildConfig {
                subject: Some(HEX64.into()),
                limits: Some(sample()),
                learning: None,
            },
        )
        .unwrap();
        let learning = charter_proto::GrantLearning::from_value(&serde_json::json!({
            "v": 1, "issuedAt": 7,
            "apps": [{"id": "khan-academy", "label": "Khan Academy", "kind": "site",
                       "url": "https://www.khanacademy.org/", "domains": ["khanacademy.org"]}]
        }))
        .unwrap();
        set_child_learning(d, "alice", Some(&learning)).unwrap();
        let cfg =
            parse_child_config(&std::fs::read_to_string(child_limits_path(d, "alice")).unwrap())
                .unwrap();
        assert_eq!(cfg.subject.as_deref(), Some(HEX64), "binding survives");
        assert!(cfg.limits.is_some(), "limits survive");
        assert_eq!(cfg.learning.as_ref().unwrap().apps[0].id, "khan-academy");
        // Clearing removes learning but nothing else.
        set_child_learning(d, "alice", None).unwrap();
        let cfg =
            parse_child_config(&std::fs::read_to_string(child_limits_path(d, "alice")).unwrap())
                .unwrap();
        assert!(cfg.learning.is_none());
        assert!(cfg.limits.is_some() && cfg.subject.is_some());
    }

    #[test]
    fn set_child_limits_creates_unbound_child_when_no_file() {
        // First-time device-only setup (no existing file): a plain limits child.
        let dir = std::env::temp_dir().join(format!("charter-scl2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();
        set_child_limits(d, "bob", &sample()).unwrap();
        let cfg =
            parse_child_config(&std::fs::read_to_string(child_limits_path(d, "bob")).unwrap())
                .unwrap();
        assert_eq!(cfg.subject, None);
        assert_eq!(cfg.limits.as_ref().unwrap().daily_minutes, 120);
    }

    #[test]
    fn set_child_subject_rejects_bad_hex() {
        let dir = std::env::temp_dir().join(format!("charter-cc2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(set_child_subject(dir.to_str().unwrap(), "alice", Some("xyz")).is_err());
    }

    #[test]
    fn load_child_limits_dir_surfaces_structured_limits_too() {
        let dir = std::env::temp_dir().join(format!("charter-cc3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_str().unwrap();
        // A structured (bound + limits) child and a binding-only child.
        save_child_config(
            &child_limits_path(d, "alice"),
            &ChildConfig {
                subject: Some(HEX64.into()),
                limits: Some(sample()),
                learning: None,
            },
        )
        .unwrap();
        save_child_config(
            &child_limits_path(d, "bob"),
            &ChildConfig {
                subject: Some(HEX64.into()),
                limits: None,
                learning: None,
            },
        )
        .unwrap();
        // The GUI view shows only children that have device-only limits (alice).
        let limits = load_child_limits_dir(d);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].0, "alice");
        // But the full config loader sees both.
        assert_eq!(load_child_configs(d).len(), 2);
    }

    #[test]
    fn tz_name_shape_accepts_ordinary_iana_names() {
        assert!(tz_name_shape_ok("UTC"));
        assert!(tz_name_shape_ok("Europe/London"));
        assert!(tz_name_shape_ok("America/Argentina/Buenos_Aires"));
        assert!(tz_name_shape_ok("Etc/GMT+1"));
        assert!(tz_name_shape_ok("Etc/GMT-1"));
    }

    #[test]
    fn tz_name_shape_rejects_empty_and_malformed() {
        assert!(!tz_name_shape_ok(""));
        // timedatectl's own "unknown" sentinel — must never be treated as a
        // zone name even though it happens to pass a naive alnum check.
        assert!(tz_name_shape_ok("n/a")); // shape alone can't catch this...
                                           // ...which is exactly why `tz_is_valid` also requires the
                                           // zoneinfo file to exist (covered by the doc comment above
                                           // `tz_is_valid`; not re-asserted here since it needs a real
                                           // /usr/share/zoneinfo on the test machine).
        assert!(!tz_name_shape_ok("/leading/slash/empty/segment"));
        assert!(!tz_name_shape_ok("trailing/slash/"));
        assert!(!tz_name_shape_ok("has space"));
        assert!(!tz_name_shape_ok("../../etc/passwd"));
        assert!(!tz_name_shape_ok("semi;colon"));
    }

    #[test]
    fn tz_is_valid_requires_a_real_zoneinfo_file() {
        // Only run where tzdata is actually installed (true on every Debian/
        // Ubuntu box this ships to, but not guaranteed in every CI image).
        if !std::path::Path::new("/usr/share/zoneinfo/UTC").exists() {
            return;
        }
        assert!(tz_is_valid("UTC"));
        assert!(tz_is_valid("Europe/London"));
        assert!(!tz_is_valid("n/a"));
        assert!(!tz_is_valid("Not/A_Real_Zone"));
        assert!(!tz_is_valid(""));
    }
}
