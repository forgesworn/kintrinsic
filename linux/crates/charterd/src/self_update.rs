//! Guardian-triggered self-update of the Linux ward (contract § "Update —
//! self-update", "On Linux (from 0.7.12)").
//!
//! The guardian taps **Install update**; the `update` clause names the
//! `kintrinsic` package at a `versionCode`, the `.deb`'s sha256 and the
//! release key. `charterd` installs only when that clause AND a kind-30063
//! `charter-deb` announcement verified against the compiled-in release key
//! name the same `versionCode` and the same sha256 — two keys must agree.
//! Without a standing clause naming a newer version nothing installs: the
//! staged `.deb` from [`crate::release_check`] just waits.
//!
//! Everything that decides is pure and tested under `mock`: [`plan`],
//! [`agreeing_release`], [`pick_source`], [`record_attempt`] and
//! [`update_health`], plus the async [`update_round`] that strings them
//! together over injected relay, fetch and [`Installer`] seams. The only real
//! side effect, `systemd-run`, sits behind the `real` feature in
//! [`real::SystemdRun`].
//!
//! The install runs outside `charterd`'s own unit because the package's
//! `postinst` restarts `charterd`: a child of this service would be killed
//! half-way through `dpkg`.

use std::fs;
use std::path::{Path, PathBuf};

use charter_primitives::{kinds, PubKey};
use charter_proto::status::UpdateHealth;
use charter_proto::{UpdateAppBody, CLAUSE_VERSION};
use charter_sys::relay::{Filter, RelayTransport};
use charter_verify::software_release::{verify_software_release, SoftwareRelease};

use crate::release_check::{
    ensure_private_dir, stage_bytes, staged_path, CHANNEL, MAX_ARTIFACT_BYTES, RELEASE_RELAYS,
};

/// The Debian package the Linux clause must name. Any other value (an
/// Android `org.forgesworn.charter` clause, say) is refused.
pub const DEB_PACKAGE: &str = "kintrinsic";

/// At most one attempt per `versionCode` in this window.
pub const ATTEMPT_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// The attempt record, next to the staged `.deb` in the root-only
/// [`crate::release_check::UPDATES_DIR`]. Durable on purpose: a successful
/// install restarts `charterd` (postinst), and `Restart=always` restarts it
/// on anything else, so an in-memory count would retry on every restart.
pub const ATTEMPTS_FILE: &str = "attempts.json";

/// Why a clause leads to no attempt at all. None of these is an install
/// failure, so none is reported in STATUS `updateHealth` (as on Android,
/// where a directive that fails its gates is never parked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// No `update` clause is stored.
    NoClause,
    /// Unknown body version, `versionCode` 0, or a non-https `url`.
    BadBody(&'static str),
    /// `packageName` is not [`DEB_PACKAGE`].
    WrongPackage,
    /// `signerCertSha256` is not the compiled-in release key.
    WrongReleaseKey,
    /// Installed `versionCode` equals the clause's: nothing to do.
    UpToDate,
    /// The clause names a version below the installed one.
    Downgrade,
}

/// What the gates say about the stored clause right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Skip(Skip),
    /// Gates pass, but this `versionCode` was attempted under
    /// [`ATTEMPT_INTERVAL_SECS`] ago.
    Wait {
        next_at: u64,
    },
    /// Gates pass and an attempt is due.
    Attempt,
}

/// The clause gates, in the order the contract states them. `body.validate()`
/// is deliberately NOT used: its Android package-name rule needs a dotted
/// name and would refuse the spec-correct `kintrinsic`.
pub fn check_clause(body: &UpdateAppBody, installed: u64, pinned: &PubKey) -> Result<(), Skip> {
    if body.v != CLAUSE_VERSION {
        return Err(Skip::BadBody("unknown body version"));
    }
    if body.package_name != DEB_PACKAGE {
        return Err(Skip::WrongPackage);
    }
    if body.version_code == 0 {
        return Err(Skip::BadBody("versionCode must be positive"));
    }
    if !body.url.starts_with("https://") {
        return Err(Skip::BadBody("url must be https"));
    }
    if body.signer_cert_sha256.to_hex() != pinned.to_hex() {
        return Err(Skip::WrongReleaseKey);
    }
    if body.version_code < installed {
        return Err(Skip::Downgrade);
    }
    if body.version_code == installed {
        return Err(Skip::UpToDate);
    }
    Ok(())
}

/// Gates, then the attempt window.
pub fn plan(
    clause: Option<&UpdateAppBody>,
    installed: u64,
    pinned: &PubKey,
    history: Option<&AttemptRecord>,
    now: u64,
) -> Plan {
    let Some(body) = clause else {
        return Plan::Skip(Skip::NoClause);
    };
    if let Err(skip) = check_clause(body, installed, pinned) {
        return Plan::Skip(skip);
    }
    if let Some(rec) = history.filter(|r| r.version_code == body.version_code) {
        let next_at = rec.last_at.saturating_add(ATTEMPT_INTERVAL_SECS);
        if now < next_at {
            return Plan::Wait { next_at };
        }
    }
    Plan::Attempt
}

/// Why the release key's announcement does not back the guardian's clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disagreement {
    /// No verified announcement names the clause's `versionCode`.
    NoAnnouncement,
    /// One does, with a different sha256.
    ShaMismatch,
}

impl Disagreement {
    pub fn describe(&self, version_code: u64) -> String {
        match self {
            Disagreement::NoAnnouncement => {
                format!("no signed release announcement names version {version_code}")
            }
            Disagreement::ShaMismatch => format!(
                "the signed release announcement for version {version_code} names a different file"
            ),
        }
    }
}

/// The verified announcement that backs `clause`: same `versionCode`, same
/// sha256. `releases` must already be verified against the release key (the
/// caller runs [`verify_software_release`]); an announcement over the
/// artifact cap is never a match.
pub fn agreeing_release<'a>(
    clause: &UpdateAppBody,
    releases: &'a [SoftwareRelease],
) -> Result<&'a SoftwareRelease, Disagreement> {
    let same_version: Vec<&SoftwareRelease> = releases
        .iter()
        .filter(|r| r.version_code == clause.version_code && r.size_bytes <= MAX_ARTIFACT_BYTES)
        .collect();
    if same_version.is_empty() {
        return Err(Disagreement::NoAnnouncement);
    }
    same_version
        .into_iter()
        .find(|r| r.sha256 == clause.apk_sha256)
        .ok_or(Disagreement::ShaMismatch)
}

/// Where the bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The release check's staged file, whose sha256 matches.
    Staged(PathBuf),
    /// Fetch these in order, each under the announcement's sha256 + size.
    Fetch(Vec<String>),
}

/// The staged file when its bytes hash to the clause's sha256; otherwise the
/// clause's `url`, then the Blossom address(es) of the hash the announcement
/// names (a url whose path carries the sha256 hex).
pub fn pick_source(
    clause: &UpdateAppBody,
    release: &SoftwareRelease,
    staged: &Path,
    staged_sha: Option<[u8; 32]>,
) -> Source {
    if staged_sha.as_ref() == Some(clause.apk_sha256.as_bytes()) {
        return Source::Staged(staged.to_path_buf());
    }
    Source::Fetch(fetch_urls(clause, release))
}

/// The clause's `url` first, then the announcement's Blossom addresses of
/// the hash, without repeats.
pub fn fetch_urls(clause: &UpdateAppBody, release: &SoftwareRelease) -> Vec<String> {
    let hex = clause.apk_sha256.to_hex();
    let mut out = vec![clause.url.clone()];
    for u in &release.urls {
        if u.starts_with("https://") && u.contains(&hex) && !out.contains(u) {
            out.push(u.clone());
        }
    }
    out
}

/// One `versionCode`'s attempt history, persisted at [`ATTEMPTS_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptRecord {
    pub version_code: u64,
    pub attempts: u32,
    /// When the first attempt at this version was made (unix secs).
    pub since_unix: u64,
    /// When the latest attempt was made (unix secs): the window's anchor.
    pub last_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Count a new attempt at `version_code`. A record for another version is
/// replaced. A previous attempt that launched cleanly but left this machine
/// on the old version gets the error it never had the chance to report (its
/// restart never came).
pub fn record_attempt(prev: Option<AttemptRecord>, version_code: u64, now: u64) -> AttemptRecord {
    match prev {
        Some(mut r) if r.version_code == version_code => {
            if r.last_error.is_none() {
                r.last_error = Some("the last install did not complete".into());
            }
            r.attempts = r.attempts.saturating_add(1);
            r.last_at = now;
            r
        }
        _ => AttemptRecord {
            version_code,
            attempts: 1,
            since_unix: now,
            last_at: now,
            last_error: None,
        },
    }
}

/// STATUS `updateHealth`, exactly as on Android: present while an attempt
/// has been made at a version this machine is not yet running.
pub fn update_health(rec: Option<&AttemptRecord>, installed: u64) -> Option<UpdateHealth> {
    let r = rec?;
    if r.attempts == 0 || r.version_code <= installed {
        return None;
    }
    Some(UpdateHealth {
        package_name: DEB_PACKAGE.into(),
        version_code: Some(r.version_code),
        attempts: r.attempts,
        since_unix: r.since_unix,
        last_error: r.last_error.clone(),
    })
}

/// The persisted record, if any (unreadable = none).
pub fn load_record(dir: &Path) -> Option<AttemptRecord> {
    let raw = fs::read_to_string(dir.join(ATTEMPTS_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Persist the record, root-only.
pub fn save_record(dir: &Path, rec: &AttemptRecord) {
    if ensure_private_dir(dir).is_err() {
        eprintln!("charterd: update: cannot prepare {}", dir.display());
        return;
    }
    let path = dir.join(ATTEMPTS_FILE);
    let json = serde_json::to_vec(rec).expect("attempt record serializes");
    if let Err(e) = crate::atomic_file::atomic_write(&path.to_string_lossy(), &json, 0o600) {
        eprintln!("charterd: update: cannot write {}: {e}", path.display());
    }
}

/// Forget the record once this machine runs that version (or newer).
pub fn clear_landed(dir: &Path, installed: u64) {
    if load_record(dir).is_some_and(|r| r.version_code <= installed) {
        let _ = fs::remove_file(dir.join(ATTEMPTS_FILE));
    }
}

/// Among every managed ward's stored `update` body, the one to act on: a
/// parseable [`DEB_PACKAGE`] body with the highest `versionCode`, else any
/// parseable body (so its refusal is logged). A laptop shared by two wards
/// holds one clause per ward; the machine is updated once.
pub fn pick_clause<I: IntoIterator<Item = String>>(raw: I) -> Option<UpdateAppBody> {
    let bodies: Vec<UpdateAppBody> = raw
        .into_iter()
        .filter_map(|s| serde_json::from_str::<UpdateAppBody>(&s).ok())
        .collect();
    bodies
        .iter()
        .filter(|b| b.package_name == DEB_PACKAGE)
        .max_by_key(|b| b.version_code)
        .or_else(|| bodies.first())
        .cloned()
}

/// Starts the install. The real one is [`real::SystemdRun`]; tests use a
/// recorder. Returning `Ok` means the transient unit was started, not that
/// the install succeeded: success is the restarted `charterd` reporting the
/// new version.
pub trait Installer {
    fn launch(&self, version_code: u64, deb: &Path) -> Result<(), String>;
}

/// The exact command: a transient unit outside `charterd`'s service, so the
/// package's postinst can restart `charterd` without killing `dpkg`.
pub fn install_argv(version_code: u64, deb: &Path) -> Vec<String> {
    vec![
        "systemd-run".into(),
        format!("--unit=kintrinsic-update-{version_code}"),
        "--collect".into(),
        "--setenv=DEBIAN_FRONTEND=noninteractive".into(),
        "apt-get".into(),
        "install".into(),
        "-y".into(),
        deb.to_string_lossy().into_owned(),
    ]
}

/// What one round did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoundOutcome {
    Skipped(Skip),
    Waiting { next_at: u64 },
    Refused(Disagreement),
    FetchFailed,
    Launched { version_code: u64, path: PathBuf },
    LaunchFailed(String),
}

/// One round: gates → attempt window → (count the attempt) → the release
/// key's announcement for the same version → staged bytes or a pinned fetch
/// → [`Installer::launch`]. Every failure after the gates is written to the
/// attempt record, which is what STATUS reports.
#[allow(clippy::too_many_arguments)]
pub async fn update_round<T, F, Fut, I>(
    clause: Option<&UpdateAppBody>,
    installed: u64,
    now: u64,
    pinned: &PubKey,
    transport: &T,
    dir: &Path,
    fetch: F,
    installer: &I,
) -> RoundOutcome
where
    T: RelayTransport + ?Sized,
    F: Fn(String, u64) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
    I: Installer + ?Sized,
{
    clear_landed(dir, installed);
    let history = load_record(dir);
    match plan(clause, installed, pinned, history.as_ref(), now) {
        Plan::Skip(skip) => return RoundOutcome::Skipped(skip),
        Plan::Wait { next_at } => return RoundOutcome::Waiting { next_at },
        Plan::Attempt => {}
    }
    let clause = clause.expect("plan attempts only with a clause");
    let vc = clause.version_code;
    let mut rec = record_attempt(history, vc, now);
    save_record(dir, &rec);

    let releases = query_releases(transport, pinned).await;
    let release = match agreeing_release(clause, &releases) {
        Ok(r) => r.clone(),
        Err(d) => {
            let why = d.describe(vc);
            eprintln!("charterd: update {}: refused — {why}", clause.version_name);
            rec.last_error = Some(why);
            save_record(dir, &rec);
            return RoundOutcome::Refused(d);
        }
    };

    let staged = staged_path(dir, &release);
    let staged_sha = fs::read(&staged).ok().map(|b| charter_crypto::sha256(&b));
    let path = match pick_source(clause, &release, &staged, staged_sha) {
        Source::Staged(p) => p,
        Source::Fetch(urls) => {
            let mut got = None;
            for url in urls {
                match fetch(url.clone(), release.size_bytes.min(MAX_ARTIFACT_BYTES)).await {
                    Ok(bytes) => {
                        if let Some(s) = stage_bytes(&bytes, &release, dir) {
                            got = Some(s.path);
                            break;
                        }
                    }
                    Err(why) => eprintln!("charterd: update mirror {url}: {why}"),
                }
            }
            match got {
                Some(p) => p,
                None => {
                    rec.last_error = Some(format!(
                        "could not download version {} with the signed sha256",
                        release.version_name
                    ));
                    save_record(dir, &rec);
                    return RoundOutcome::FetchFailed;
                }
            }
        }
    };

    match installer.launch(vc, &path) {
        Ok(()) => {
            eprintln!(
                "charterd: update {}: installing {} (the guardian asked for it)",
                release.version_name,
                path.display()
            );
            RoundOutcome::Launched {
                version_code: vc,
                path,
            }
        }
        Err(e) => {
            eprintln!(
                "charterd: update {}: install did not start — {e}",
                release.version_name
            );
            rec.last_error = Some(e.clone());
            save_record(dir, &rec);
            RoundOutcome::LaunchFailed(e)
        }
    }
}

/// Every `charter-deb` announcement the release relays hold that verifies
/// against `pinned`. Transport failure is an empty list.
pub async fn query_releases<T: RelayTransport + ?Sized>(
    transport: &T,
    pinned: &PubKey,
) -> Vec<SoftwareRelease> {
    let relays: Vec<String> = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
    let filter = Filter {
        kinds: vec![kinds::SOFTWARE_RELEASE],
        authors: vec![*pinned],
        ..Filter::default()
    };
    transport
        .query(&relays, filter)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|ev| verify_software_release(ev, pinned, CHANNEL).ok())
        .collect()
}

#[cfg(feature = "real")]
pub mod real {
    //! The `systemd-run` installer and the background task.

    use super::*;
    use std::sync::{Arc, Mutex};

    use charter_sys::relay::RealRelayTransport;
    use tokio::sync::watch;

    use crate::release_check::{pinned_release_key, UPDATES_DIR};

    /// Starts [`install_argv`] and reports whether the unit started.
    pub struct SystemdRun;

    impl Installer for SystemdRun {
        fn launch(&self, version_code: u64, deb: &Path) -> Result<(), String> {
            let argv = install_argv(version_code, deb);
            let out = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .output()
                .map_err(|e| format!("systemd-run: {e}"))?;
            if out.status.success() {
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(format!(
                    "systemd-run failed: {}",
                    err.chars().take(200).collect::<String>()
                ))
            }
        }
    }

    /// Re-check this often even without a new clause: the attempt window
    /// lapses, a release gets announced.
    pub const RECHECK_SECS: u64 = 10 * 60;

    /// The enforcement loop's handle: it hands in the stored clause and
    /// reads back the health line for STATUS.
    #[derive(Clone)]
    pub struct UpdaterHandle {
        clause: watch::Sender<Option<UpdateAppBody>>,
        health: Arc<Mutex<Option<UpdateHealth>>>,
    }

    impl UpdaterHandle {
        /// Hand in the current clause; wakes the task only on a change.
        pub fn set_clause(&self, body: Option<UpdateAppBody>) {
            self.clause.send_if_modified(|cur| {
                if *cur == body {
                    false
                } else {
                    *cur = body;
                    true
                }
            });
        }

        /// The current `updateHealth` for STATUS (machine-wide).
        pub fn health(&self) -> Option<UpdateHealth> {
            match self.health.lock() {
                Ok(h) => h.clone(),
                Err(_) => None,
            }
        }
    }

    /// Spawn the updater on its own task and transport: a relay query, a
    /// 13 MB download and `systemd-run` must never sit on the watchdog-timed
    /// enforcement tick.
    pub fn spawn_updater() -> UpdaterHandle {
        let (tx, mut rx) = watch::channel::<Option<UpdateAppBody>>(None);
        let dir = PathBuf::from(UPDATES_DIR);
        let installed = crate::version::version_code();
        let health = Arc::new(Mutex::new(update_health(
            load_record(&dir).as_ref(),
            installed,
        )));
        let handle = UpdaterHandle {
            clause: tx,
            health: health.clone(),
        };
        tokio::spawn(async move {
            let transport = RealRelayTransport::default();
            let pinned = pinned_release_key();
            let mut last: Option<RoundOutcome> = None;
            loop {
                let clause = rx.borrow_and_update().clone();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let outcome = update_round(
                    clause.as_ref(),
                    installed,
                    now,
                    &pinned,
                    &transport,
                    &dir,
                    crate::release_check::real::fetch_blob,
                    &SystemdRun,
                )
                .await;
                if let RoundOutcome::Skipped(skip) = &outcome {
                    // A refused clause is logged once, not every ten minutes.
                    let quiet = matches!(skip, Skip::NoClause | Skip::UpToDate);
                    if !quiet && last.as_ref() != Some(&outcome) {
                        eprintln!("charterd: update clause not acted on: {skip:?}");
                    }
                }
                last = Some(outcome);
                if let Ok(mut h) = health.lock() {
                    *h = update_health(load_record(&dir).as_ref(), installed);
                }
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            return; // the daemon dropped its handle
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(RECHECK_SECS)) => {}
                }
            }
        });
        handle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::Sha256Hex;

    const NOW: u64 = 1_800_000_000;
    const SHA: [u8; 32] = [0x5a; 32];

    fn key() -> PubKey {
        PubKey::from_hex(&"11".repeat(32)).unwrap()
    }

    fn body(version_code: u64) -> UpdateAppBody {
        UpdateAppBody {
            v: 1,
            package_name: DEB_PACKAGE.into(),
            version_code,
            version_name: format!("0.7.{}", version_code - 700),
            url: "https://github.com/forgesworn/kintrinsic/releases/download/x.deb".into(),
            apk_sha256: Sha256Hex::from_bytes(SHA),
            signer_cert_sha256: Sha256Hex::from_hex(&key().to_hex()).unwrap(),
        }
    }

    fn release(version_code: u64, sha: [u8; 32]) -> SoftwareRelease {
        let hex = Sha256Hex::from_bytes(sha).to_hex();
        SoftwareRelease {
            channel: CHANNEL.into(),
            version_name: format!("0.7.{}", version_code - 700),
            version_code,
            sha256: Sha256Hex::from_bytes(sha),
            size_bytes: 10,
            urls: vec![
                "https://github.com/forgesworn/kintrinsic/releases/download/x.deb".into(),
                format!("https://nostr.download/{hex}.deb"),
                "https://cdn.example/unrelated".into(),
            ],
            cert_sha256: None,
            created_at: NOW,
        }
    }

    #[test]
    fn a_good_clause_above_the_installed_version_is_attempted() {
        assert_eq!(
            plan(Some(&body(712)), 711, &key(), None, NOW),
            Plan::Attempt
        );
    }

    #[test]
    fn no_clause_means_nothing_installs() {
        assert_eq!(
            plan(None, 711, &key(), None, NOW),
            Plan::Skip(Skip::NoClause)
        );
    }

    #[test]
    fn a_wrong_package_name_is_refused() {
        let mut b = body(712);
        b.package_name = "org.forgesworn.charter".into();
        assert_eq!(
            plan(Some(&b), 711, &key(), None, NOW),
            Plan::Skip(Skip::WrongPackage)
        );
    }

    #[test]
    fn a_clause_naming_another_release_key_is_refused() {
        let mut b = body(712);
        b.signer_cert_sha256 = Sha256Hex::from_bytes([0x22; 32]);
        assert_eq!(
            plan(Some(&b), 711, &key(), None, NOW),
            Plan::Skip(Skip::WrongReleaseKey)
        );
    }

    #[test]
    fn bad_bodies_are_refused() {
        let mut b = body(712);
        b.v = 2;
        assert!(matches!(
            plan(Some(&b), 711, &key(), None, NOW),
            Plan::Skip(Skip::BadBody(_))
        ));
        let mut b = body(712);
        b.url = "http://x.example/a.deb".into();
        assert!(matches!(
            plan(Some(&b), 711, &key(), None, NOW),
            Plan::Skip(Skip::BadBody(_))
        ));
        let mut b = body(712);
        b.version_code = 0;
        assert!(matches!(
            plan(Some(&b), 0, &key(), None, NOW),
            Plan::Skip(Skip::BadBody(_))
        ));
    }

    #[test]
    fn nothing_happens_at_the_clause_version() {
        assert_eq!(
            plan(Some(&body(712)), 712, &key(), None, NOW),
            Plan::Skip(Skip::UpToDate)
        );
    }

    #[test]
    fn a_downgrade_is_refused() {
        assert_eq!(
            plan(Some(&body(712)), 713, &key(), None, NOW),
            Plan::Skip(Skip::Downgrade)
        );
    }

    #[test]
    fn at_most_one_attempt_per_version_every_six_hours() {
        let rec = record_attempt(None, 712, NOW);
        assert_eq!(
            plan(Some(&body(712)), 711, &key(), Some(&rec), NOW + 60),
            Plan::Wait {
                next_at: NOW + ATTEMPT_INTERVAL_SECS
            }
        );
        assert_eq!(
            plan(
                Some(&body(712)),
                711,
                &key(),
                Some(&rec),
                NOW + ATTEMPT_INTERVAL_SECS - 1
            ),
            Plan::Wait {
                next_at: NOW + ATTEMPT_INTERVAL_SECS
            }
        );
        assert_eq!(
            plan(
                Some(&body(712)),
                711,
                &key(),
                Some(&rec),
                NOW + ATTEMPT_INTERVAL_SECS
            ),
            Plan::Attempt
        );
        // Another version is not held by this one's window.
        assert_eq!(
            plan(Some(&body(713)), 711, &key(), Some(&rec), NOW + 60),
            Plan::Attempt
        );
    }

    #[test]
    fn two_keys_must_agree() {
        let b = body(712);
        // Same version, same sha: the backing announcement.
        let rels = vec![release(713, SHA), release(712, SHA)];
        assert_eq!(agreeing_release(&b, &rels).unwrap().version_code, 712);
        // Same version, different file.
        assert_eq!(
            agreeing_release(&b, &[release(712, [0x01; 32])]),
            Err(Disagreement::ShaMismatch)
        );
        // Only a newer (or no) announcement: the release key never named this.
        assert_eq!(
            agreeing_release(&b, &[release(713, SHA)]),
            Err(Disagreement::NoAnnouncement)
        );
        assert_eq!(agreeing_release(&b, &[]), Err(Disagreement::NoAnnouncement));
        // An oversize announcement is never a match.
        let mut big = release(712, SHA);
        big.size_bytes = MAX_ARTIFACT_BYTES + 1;
        assert_eq!(
            agreeing_release(&b, &[big]),
            Err(Disagreement::NoAnnouncement)
        );
    }

    #[test]
    fn the_staged_file_is_used_only_when_its_sha_matches() {
        let b = body(712);
        let r = release(712, SHA);
        let p = Path::new("/var/lib/charter/updates/charter_0.7.12_amd64.deb");
        assert_eq!(
            pick_source(&b, &r, p, Some(SHA)),
            Source::Staged(p.to_path_buf())
        );
        let hex = Sha256Hex::from_bytes(SHA).to_hex();
        let fetch = Source::Fetch(vec![
            b.url.clone(),
            format!("https://nostr.download/{hex}.deb"),
        ]);
        assert_eq!(pick_source(&b, &r, p, Some([0x01; 32])), fetch);
        assert_eq!(pick_source(&b, &r, p, None), fetch);
    }

    #[test]
    fn health_reports_attempts_at_a_version_not_yet_running() {
        assert_eq!(update_health(None, 711), None);
        let mut rec = record_attempt(None, 712, NOW);
        rec.last_error = Some("boom".into());
        let h = update_health(Some(&rec), 711).expect("health");
        assert_eq!(h.package_name, DEB_PACKAGE);
        assert_eq!(h.version_code, Some(712));
        assert_eq!(h.attempts, 1);
        assert_eq!(h.since_unix, NOW);
        assert_eq!(h.last_error.as_deref(), Some("boom"));
        // Landed: nothing to report.
        assert_eq!(update_health(Some(&rec), 712), None);
    }

    #[test]
    fn a_repeat_attempt_counts_and_says_the_last_one_did_not_land() {
        let first = record_attempt(None, 712, NOW);
        assert_eq!(first.attempts, 1);
        assert_eq!(first.last_error, None);
        let second = record_attempt(Some(first), 712, NOW + ATTEMPT_INTERVAL_SECS);
        assert_eq!(second.attempts, 2);
        assert_eq!(second.since_unix, NOW);
        assert_eq!(second.last_at, NOW + ATTEMPT_INTERVAL_SECS);
        assert!(second.last_error.is_some());
        // A new version starts over.
        let other = record_attempt(Some(second), 713, NOW + 1);
        assert_eq!((other.attempts, other.since_unix), (1, NOW + 1));
    }

    #[test]
    fn the_install_runs_in_its_own_transient_unit() {
        let argv = install_argv(712, Path::new("/var/lib/charter/updates/k.deb"));
        assert_eq!(
            argv,
            [
                "systemd-run",
                "--unit=kintrinsic-update-712",
                "--collect",
                "--setenv=DEBIAN_FRONTEND=noninteractive",
                "apt-get",
                "install",
                "-y",
                "/var/lib/charter/updates/k.deb",
            ]
        );
    }

    #[test]
    fn pick_clause_prefers_the_newest_deb_clause() {
        let android = serde_json::to_string(&UpdateAppBody {
            package_name: "org.forgesworn.charter".into(),
            version_code: 99_999,
            ..body(712)
        })
        .unwrap();
        let a = serde_json::to_string(&body(712)).unwrap();
        let b = serde_json::to_string(&body(713)).unwrap();
        let got = pick_clause([android.clone(), a, "garbage".into(), b]).unwrap();
        assert_eq!(got.version_code, 713);
        // Only a foreign clause: returned so its refusal is visible.
        assert_eq!(
            pick_clause([android]).unwrap().package_name,
            "org.forgesworn.charter"
        );
        assert_eq!(pick_clause(Vec::<String>::new()), None);
    }

    #[test]
    fn the_spec_shape_parses_with_the_deb_package_name() {
        let json = format!(
            r#"{{"v":1,"packageName":"kintrinsic","versionCode":712,"versionName":"0.7.12","url":"https://x.example/k.deb","apkSha256":"{}","signerCertSha256":"{}"}}"#,
            "ab".repeat(32),
            key().to_hex()
        );
        let b: UpdateAppBody = serde_json::from_str(&json).unwrap();
        assert_eq!(check_clause(&b, 711, &key()), Ok(()));
    }
}

#[cfg(all(test, feature = "mock"))]
mod round_tests {
    use super::*;
    use charter_primitives::{NostrEvent, Sha256Hex};
    use charter_sys::relay::MockRelayTransport;
    use charter_sys::signer::{MachineSigner, SeedSigner};
    use std::cell::RefCell;

    const NOW: u64 = 1_800_000_000;

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<(u64, PathBuf)>>,
        fail: Option<String>,
    }

    impl Installer for Recorder {
        fn launch(&self, version_code: u64, deb: &Path) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push((version_code, deb.to_path_buf()));
            match &self.fail {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    fn signer() -> SeedSigner {
        SeedSigner::from_seed(0x11)
    }

    fn pin() -> PubKey {
        MachineSigner::pubkey(&signer())
    }

    fn announce(s: &SeedSigner, version_code: u64, bytes: &[u8]) -> NostrEvent {
        let sha = Sha256Hex::from_bytes(charter_crypto::sha256(bytes));
        let tags = vec![
            vec!["d".into(), CHANNEL.into()],
            vec!["version".into(), format!("0.7.{}", version_code - 700)],
            vec!["version_code".into(), version_code.to_string()],
            vec!["x".into(), sha.to_hex()],
            vec!["size".into(), bytes.len().to_string()],
            vec![
                "url".into(),
                "https://github.com/forgesworn/kintrinsic/x.deb".into(),
            ],
            vec![
                "url".into(),
                format!("https://nostr.download/{}.deb", sha.to_hex()),
            ],
        ];
        let mut ev = NostrEvent {
            id: charter_primitives::EventId::from_bytes([0; 32]),
            pubkey: MachineSigner::pubkey(s),
            created_at: NOW,
            kind: kinds::SOFTWARE_RELEASE,
            tags,
            content: String::new(),
            sig: charter_primitives::Sig::from_bytes([0; 64]),
        };
        ev.id = charter_verify::event::event_id(&ev);
        ev.sig = MachineSigner::sign(s, ev.id.as_bytes()).expect("sign");
        ev
    }

    fn clause(version_code: u64, bytes: &[u8]) -> UpdateAppBody {
        UpdateAppBody {
            v: 1,
            package_name: DEB_PACKAGE.into(),
            version_code,
            version_name: format!("0.7.{}", version_code - 700),
            url: "https://github.com/forgesworn/kintrinsic/x.deb".into(),
            apk_sha256: Sha256Hex::from_bytes(charter_crypto::sha256(bytes)),
            signer_cert_sha256: Sha256Hex::from_hex(&pin().to_hex()).unwrap(),
        }
    }

    async fn relay_with(events: Vec<NostrEvent>) -> MockRelayTransport {
        let relay = MockRelayTransport::new();
        let relays: Vec<String> = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
        for ev in events {
            let _ = relay.publish(&relays, ev).await;
        }
        relay
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "charterd-self-update-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn serve(
        bytes: Vec<u8>,
    ) -> impl Fn(String, u64) -> std::future::Ready<Result<Vec<u8>, String>> {
        move |_url, _cap| std::future::ready(Ok(bytes.clone()))
    }

    #[tokio::test]
    async fn agreeing_keys_fetch_stage_and_launch_once_per_window() {
        let bytes = b"the 0.7.12 deb".to_vec();
        let relay = relay_with(vec![announce(&signer(), 712, &bytes)]).await;
        let dir = tmpdir("ok");
        let inst = Recorder::default();
        let c = clause(712, &bytes);

        let out = update_round(
            Some(&c),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes.clone()),
            &inst,
        )
        .await;
        let RoundOutcome::Launched { version_code, path } = out.clone() else {
            panic!("expected a launch, got {out:?}");
        };
        assert_eq!(version_code, 712);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(inst.calls.borrow().len(), 1);
        // The record is durable and is what STATUS reports.
        let rec = load_record(&dir).expect("persisted");
        assert_eq!((rec.version_code, rec.attempts), (712, 1));
        assert_eq!(update_health(Some(&rec), 711).unwrap().attempts, 1);

        // Same window: no second attempt, however often the round runs.
        let again = update_round(
            Some(&c),
            711,
            NOW + 600,
            &pin(),
            &relay,
            &dir,
            serve(bytes.clone()),
            &inst,
        )
        .await;
        assert!(matches!(again, RoundOutcome::Waiting { .. }));
        assert_eq!(inst.calls.borrow().len(), 1);

        // Six hours on, still not landed: one more, from the staged file.
        let later = NOW + ATTEMPT_INTERVAL_SECS;
        let fail_fetch =
            |_u: String, _c: u64| std::future::ready(Err::<Vec<u8>, _>("offline".to_string()));
        let third = update_round(
            Some(&c),
            711,
            later,
            &pin(),
            &relay,
            &dir,
            fail_fetch,
            &inst,
        )
        .await;
        assert!(matches!(third, RoundOutcome::Launched { .. }), "{third:?}");
        assert_eq!(load_record(&dir).unwrap().attempts, 2);

        // Landed: the restarted daemon reports 712, the record is cleared.
        let landed = update_round(
            Some(&c),
            712,
            later + 60,
            &pin(),
            &relay,
            &dir,
            serve(bytes),
            &inst,
        )
        .await;
        assert_eq!(landed, RoundOutcome::Skipped(Skip::UpToDate));
        assert!(load_record(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_clause_the_release_key_never_announced_installs_nothing() {
        let bytes = b"guardian-named bytes".to_vec();
        // The release key announced different bytes for 712.
        let relay = relay_with(vec![announce(&signer(), 712, b"other bytes")]).await;
        let dir = tmpdir("disagree");
        let inst = Recorder::default();
        let out = update_round(
            Some(&clause(712, &bytes)),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes),
            &inst,
        )
        .await;
        assert_eq!(out, RoundOutcome::Refused(Disagreement::ShaMismatch));
        assert!(inst.calls.borrow().is_empty());
        let h = update_health(load_record(&dir).as_ref(), 711).expect("reported");
        assert!(h.last_error.unwrap().contains("different file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_announcement_from_another_key_does_not_count() {
        let bytes = b"forged".to_vec();
        let relay = relay_with(vec![announce(&SeedSigner::from_seed(0x99), 712, &bytes)]).await;
        let dir = tmpdir("forged");
        let inst = Recorder::default();
        let out = update_round(
            Some(&clause(712, &bytes)),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes),
            &inst,
        )
        .await;
        assert_eq!(out, RoundOutcome::Refused(Disagreement::NoAnnouncement));
        assert!(inst.calls.borrow().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn mismatched_download_bytes_are_dropped_never_installed() {
        let bytes = b"the real deb".to_vec();
        let relay = relay_with(vec![announce(&signer(), 712, &bytes)]).await;
        let dir = tmpdir("poison");
        let inst = Recorder::default();
        let out = update_round(
            Some(&clause(712, &bytes)),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(b"poisoned bytes".to_vec()),
            &inst,
        )
        .await;
        assert_eq!(out, RoundOutcome::FetchFailed);
        assert!(inst.calls.borrow().is_empty());
        assert!(load_record(&dir).unwrap().last_error.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_launch_failure_is_recorded_for_status() {
        let bytes = b"deb".to_vec();
        let relay = relay_with(vec![announce(&signer(), 712, &bytes)]).await;
        let dir = tmpdir("launchfail");
        let inst = Recorder {
            fail: Some("Unit kintrinsic-update-712.service already exists.".into()),
            ..Recorder::default()
        };
        let out = update_round(
            Some(&clause(712, &bytes)),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes),
            &inst,
        )
        .await;
        assert!(matches!(out, RoundOutcome::LaunchFailed(_)));
        let h = update_health(load_record(&dir).as_ref(), 711).unwrap();
        assert_eq!(
            h.last_error.as_deref(),
            Some("Unit kintrinsic-update-712.service already exists.")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn gates_refuse_before_any_attempt_is_counted() {
        let bytes = b"deb".to_vec();
        let relay = relay_with(vec![announce(&signer(), 712, &bytes)]).await;
        let dir = tmpdir("gates");
        let inst = Recorder::default();
        let mut wrong_key = clause(712, &bytes);
        wrong_key.signer_cert_sha256 = Sha256Hex::from_bytes([0x33; 32]);
        let out = update_round(
            Some(&wrong_key),
            711,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes.clone()),
            &inst,
        )
        .await;
        assert_eq!(out, RoundOutcome::Skipped(Skip::WrongReleaseKey));
        let out = update_round(
            Some(&clause(712, &bytes)),
            800,
            NOW,
            &pin(),
            &relay,
            &dir,
            serve(bytes),
            &inst,
        )
        .await;
        assert_eq!(out, RoundOutcome::Skipped(Skip::Downgrade));
        assert!(load_record(&dir).is_none());
        assert!(inst.calls.borrow().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_updates_dir_is_root_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("perm").join("updates");
        save_record(&dir, &record_attempt(None, 712, NOW));
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let file = fs::metadata(dir.join(ATTEMPTS_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file, 0o600);
        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }
}
