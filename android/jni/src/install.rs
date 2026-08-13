//! `install.apk` enactment (port-spec §2.4 / §3.5): the guardian-approved
//! "put this app on the phone" path. The security-critical decisions live in
//! Rust; the actual `PackageInstaller` session is Kotlin's job (it needs the
//! platform API), driven pull-based to avoid a Rust→Kotlin callback:
//!
//! 1. The broker verifies the install GRANT (consume-before-enact, single-use).
//! 2. [`InstallApkEnactor`] re-validates the **signed grant params** and parks
//!    a durable install directive in `<base>/installs/` — then returns Ok, so
//!    the grant is consumed exactly once. The queue is the only thing written
//!    by the verified-grant path (preserving I1: an install is producible only
//!    from a verified Allow).
//! 3. Kotlin drains the queue each slow tick, performs the install (verifying
//!    signing continuity against the grant's `signerCertSha256` BEFORE commit),
//!    and reports the outcome back — `ok`/`terminal` clear the entry,
//!    `transient` leaves it for the next tick (idempotent: installing an
//!    already-present same-or-newer version is a no-op success).
//!
//! Crash-safe by construction: a device that dies mid-install finds the
//! directive still queued on reboot and retries; the enactors are idempotent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use charter_proto::{GrantParams, OpType};
use charter_spine::enactor::{EnactContext, EnactOutcome, Enactor};
use charter_spine::error::EnactError;
use charter_verify::VerifiedGrant;

/// A parked, guardian-approved install directive.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInstall {
    /// The grant's reqId (dedupe key + the id Kotlin reports a result against).
    pub req_id: String,
    pub package_name: String,
    #[serde(default)]
    pub version_code: Option<u64>,
    /// The provenance the device MUST match before committing (hex).
    pub signer_cert_sha256: String,
    pub source: String,
    /// `source == "url"` only: where the device fetches the archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `source == "url"` only: sha256 the fetched bytes MUST hash to (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apk_sha256: Option<String>,
    pub at: u64,
    /// How many times the device has tried and failed. A directive that never
    /// completes is otherwise INVISIBLE to the guardian — the phone retries in
    /// silence while MyCharter shows an Update button that keeps reappearing.
    /// (Robin's phone did exactly that for two days, 2026-07-24..26.)
    #[serde(default)]
    pub attempts: u32,
    /// Why the last attempt failed, in words a parent can act on. Short and
    /// non-identifying — it goes out on STATUS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Directives older than this are swept — a week-stale approval to install an
/// app the parent has surely moved on from is not worth acting on.
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Durable queue of approved installs (file per reqId under `<base>/installs/`).
pub struct InstallQueue {
    dir: PathBuf,
}

impl InstallQueue {
    pub fn new(base: &Path) -> InstallQueue {
        let dir = base.join("installs");
        let _ = std::fs::create_dir_all(&dir);
        InstallQueue { dir }
    }

    fn path(&self, req_id: &str) -> Option<PathBuf> {
        // reqId is lowercase hex; guard so a hostile value can't traverse.
        if req_id.is_empty() || !req_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(self.dir.join(format!("{req_id}.json")))
    }

    /// Park an approved install (idempotent by reqId — a re-delivered grant
    /// does not double-queue).
    pub fn put(&self, item: &PendingInstall) {
        if let Some(p) = self.path(&item.req_id) {
            if p.exists() {
                return;
            }
            if let Ok(json) = serde_json::to_string(item) {
                let _ = std::fs::write(p, json);
            }
        }
    }

    /// Overwrite an EXISTING directive in place. Distinct from [`put`], which
    /// refuses to touch an entry that already exists (its dedupe) — so it
    /// silently drops a retry-count update. Only ever writes over a directive
    /// that is already queued; it never creates one, so it cannot be a way to
    /// smuggle an install past the verified-grant path.
    pub fn update(&self, item: &PendingInstall) {
        if let Some(p) = self.path(&item.req_id) {
            if !p.exists() {
                return;
            }
            if let Ok(json) = serde_json::to_string(item) {
                let _ = std::fs::write(p, json);
            }
        }
    }

    /// Every parked install, oldest-first, sweeping expired/corrupt entries.
    pub fn list(&self, now: u64) -> Vec<PendingInstall> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<PendingInstall> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match serde_json::from_str::<PendingInstall>(&text) {
                Ok(item) if now.saturating_sub(item.at) <= MAX_AGE_SECS => out.push(item),
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        out.sort_by_key(|i| i.at);
        out
    }

    /// Read one directive by reqId WITHOUT sweeping. Deliberately separate
    /// from [`list`], which purges stale entries as it goes — reading a single
    /// item must never be able to empty the queue as a side effect.
    pub fn get(&self, req_id: &str) -> Option<PendingInstall> {
        let path = self.path(req_id)?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<PendingInstall>(&text).ok()
    }

    /// Clear a directive (a `terminal` failure or an `ok` success).
    pub fn remove(&self, req_id: &str) {
        if let Some(p) = self.path(req_id) {
            let _ = std::fs::remove_file(p);
        }
    }

    pub fn len(&self) -> usize {
        std::fs::read_dir(&self.dir).map(|r| r.count()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The `install.apk` enactor: re-validates the signed grant params and parks a
/// durable install directive. Does NOT install itself — that's Kotlin's job,
/// drained from the queue (§2.4). Idempotent by reqId.
pub struct InstallApkEnactor {
    queue: Arc<InstallQueue>,
    now: fn() -> u64,
}

impl InstallApkEnactor {
    pub fn new(queue: Arc<InstallQueue>) -> Self {
        InstallApkEnactor {
            queue,
            now: default_now,
        }
    }
}

fn default_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait]
impl Enactor for InstallApkEnactor {
    fn op(&self) -> OpType {
        OpType::InstallApk
    }

    async fn enact(
        &self,
        grant: &VerifiedGrant,
        _ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        // Act ONLY on the signed grant params — never request params (I1).
        let params = match grant.allow_params() {
            Some(GrantParams::InstallApk(p)) => p,
            _ => return Err(EnactError::Terminal("not an install.apk allow".into())),
        };
        params
            .validate()
            .map_err(|e| EnactError::Terminal(e.to_string()))?;

        let source = match params.source {
            charter_proto::params::ApkSource::Staged => "staged",
        };
        self.queue.put(&PendingInstall {
            req_id: grant.req_id().to_hex(),
            package_name: params.package_name.clone(),
            version_code: params.version_code,
            signer_cert_sha256: params.signer_cert_sha256.to_hex(),
            source: source.to_string(),
            url: None,
            apk_sha256: None,
            at: (self.now)(),
            attempts: 0,
            last_error: None,
        });
        Ok(EnactOutcome {
            detail: Some("queued for install".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("charter-installq-{}-{}", std::process::id(), label));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn item(req: &str, pkg: &str, at: u64) -> PendingInstall {
        PendingInstall {
            req_id: req.into(),
            package_name: pkg.into(),
            version_code: Some(42),
            signer_cert_sha256: "ab".repeat(32),
            source: "staged".into(),
            url: None,
            apk_sha256: None,
            at,
            attempts: 0,
            last_error: None,
        }
    }

    #[test]
    fn queue_is_idempotent_and_ordered_and_sweeps() {
        let base = tmp("q");
        let q = InstallQueue::new(&base);
        q.put(&item("aa", "org.a.app", 100));
        q.put(&item("aa", "org.a.app", 100)); // dupe reqId — no double-queue
        q.put(&item("bb", "org.b.app", 50));
        let now = 200;
        let listed = q.list(now);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].req_id, "bb", "oldest-first");
        assert_eq!(listed[1].req_id, "aa");

        q.remove("bb");
        assert_eq!(q.list(now).len(), 1);

        // A stale directive (older than a week) is swept on list. Read the
        // queue from a vantage point well past the max age so `at=100` expires.
        let later = 100 + MAX_AGE_SECS + 1;
        assert_eq!(q.list(later).len(), 0, "expired entry swept");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn path_rejects_traversal() {
        let q = InstallQueue::new(&tmp("t"));
        assert!(q.path("../etc/passwd").is_none());
        assert!(q.path("").is_none());
        assert!(q.path("abcdef").is_some());
    }
}
