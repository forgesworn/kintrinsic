//! The `exec.allow` enactor + request builder. Authority is the **sha256** in
//! the signed grant — never a path. The enactor re-hashes the COPIED bytes
//! while relocating into the root-owned store (TOCTOU close), stores under the
//! sha256 filename, validates + desktop-escapes the display name (it never
//! reaches a path), trusts the hash in fapolicyd, and drops a launcher. The
//! consumed-id was already burned at verify; a transient enact failure may only
//! retry the in-memory verified grant.
//!
//! What the reqId binds is the guard's **resolved** path, and the enactor
//! re-opens it through the same `openat2` door the guard used
//! ([`crate::exec_guard::open_bound_candidate`]) rather than by name — on the
//! blocking pool, never on a tick worker. See that function for why: between
//! the request and the grant the ward owns every component of that path, and a
//! FIFO left at it would otherwise block the enforcer into its watchdog.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use charter_primitives::Sha256Hex;
use charter_proto::{ExecAllowParams, GrantParams, OpType};
use charter_sys::effects::{AdmitMeta, ApprovedExecStore, TrustDb};
use charter_sys::SysError;
use charter_verify::VerifiedGrant;

use crate::enactor::{EnactContext, EnactOutcome, Enactor};
use crate::error::EnactError;
use crate::exec_guard::{
    open_bound_candidate, validate_exec_candidate_open, ExecPathError, ProbeExec,
};

/// Derive a safe display name from a source path (file stem, sanitized).
pub fn sanitize_name_from_path(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.split('.').next().unwrap_or(file);
    let cleaned: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "app".to_string()
    } else {
        cleaned.chars().take(128).collect()
    }
}

/// Enacts `exec.allow` behind the approved-exec store + trust DB.
///
/// The store is held behind an `Arc` purely so the admit — an open plus a
/// streamed copy of up to 512 MiB, all of it synchronous — can be handed to
/// `spawn_blocking` without borrowing `self` into a `'static` task.
pub struct ExecAllowEnactor<S: ApprovedExecStore, T: TrustDb> {
    store: Arc<S>,
    trust: T,
}

impl<S: ApprovedExecStore + 'static, T: TrustDb> ExecAllowEnactor<S, T> {
    pub fn new(store: S, trust: T) -> Self {
        ExecAllowEnactor {
            store: Arc::new(store),
            trust,
        }
    }
    pub fn store(&self) -> &S {
        &self.store
    }
    pub fn trust(&self) -> &T {
        &self.trust
    }
}

/// How a `SysError` out of the store maps onto the broker's retry policy.
/// `Conflict` (hash mismatch / bad name), `NotFound` and `Unsupported` (over
/// the ceiling, not a regular file) are all facts about the candidate that a
/// retry cannot change.
fn admit_error(e: SysError) -> EnactError {
    match e {
        SysError::Conflict(m) => EnactError::Terminal(m),
        SysError::NotFound => EnactError::Terminal("source missing".into()),
        SysError::Unsupported(m) => EnactError::Terminal(m),
        other => EnactError::Transient(other.to_string()),
    }
}

/// How the enact-time re-open's refusal reads to the ward. All terminal: the
/// bound path is no longer the file the guardian approved, and asking the same
/// grant again will not make it one. The ward can always re-request.
fn bound_open_error(e: ExecPathError) -> EnactError {
    EnactError::Terminal(match e {
        ExecPathError::SpecialFile => {
            "the approved file is no longer an ordinary file — ask again".into()
        }
        ExecPathError::OutsideManagedTree | ExecPathError::Traversal => {
            "the approved file is no longer where it was".into()
        }
        ExecPathError::NotReadableByCaller => "the approved file cannot be read".into(),
    })
}

#[async_trait]
impl<S: ApprovedExecStore + 'static, T: TrustDb> Enactor for ExecAllowEnactor<S, T> {
    fn op(&self) -> OpType {
        OpType::ExecAllow
    }

    async fn enact(
        &self,
        grant: &VerifiedGrant,
        ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        let params = match grant.allow_params() {
            Some(GrantParams::ExecAllow(p)) => p,
            _ => return Err(EnactError::Terminal("not an exec.allow allow".into())),
        };
        let source_path = ctx
            .source_path
            .as_deref()
            .ok_or_else(|| EnactError::Terminal("no source path bound to reqId".into()))?;
        let sha = params.sha256.to_hex();

        // M8: resume-safe enact. Admit only if not already stored, then ALWAYS
        // (re)assert trust + launcher (both idempotent). The old code returned
        // Ok on `contains` BEFORE trusting, so a broker retry after a transient
        // trust failure short-circuited and left the binary stored-but-untrusted.
        let already = self
            .store
            .contains(&sha)
            .await
            .map_err(|e| EnactError::Transient(e.to_string()))?;
        if !already {
            // Relocate + re-hash the COPIED bytes; mismatch / missing / bad-name /
            // noexec are TERMINAL (nothing stored on mismatch).
            let meta = AdmitMeta {
                name: params.name.clone(),
                size: params.size,
                origin: params.origin.clone(),
            };
            // The whole open-and-copy on the BLOCKING pool, never on an
            // executor worker. The copy is synchronous by construction (it
            // streams up to 512 MiB through sha256 into the store), and
            // `ctx.source_path` names a file inside the ward's own tree, on
            // whatever filesystem they put it on — an automounted or unplugged
            // one can stall the open for seconds whatever flags it carries.
            // A tick worker stalled there is the enforcer stalled: `WatchdogSec`
            // fires, and `ExecStopPost --thaw-all` unfreezes every managed
            // child. So it goes to a blocking thread, and this task awaits the
            // join handle instead of the syscall.
            let bound = source_path.to_string();
            let store = Arc::clone(&self.store);
            let sha_for_admit = sha.clone();
            let admitted = tokio::task::spawn_blocking(move || -> Result<(), EnactError> {
                // 03-B8 at ENACT time. The grant arrives long after the
                // request, and the ward owns every component of the bound path
                // in between. Re-open it through the SAME openat2 door the
                // request-time guard used — no symlink followed, no magic
                // link, `O_NOFOLLOW | O_NONBLOCK` so a FIFO cannot park us, and
                // an `fstat` on the descriptor that only a regular file
                // survives — and copy from THAT descriptor. The path-taking
                // `admit` is not used on this route at all.
                let file = open_bound_candidate(Path::new(&bound)).map_err(bound_open_error)?;
                // The store's admit is `async` by trait shape but does no IO
                // await; driving it here keeps the bytes on this thread.
                tokio::runtime::Handle::current()
                    .block_on(store.admit_from_file(&file, &bound, &sha_for_admit, &meta))
                    .map_err(admit_error)
            })
            .await;
            match admitted {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                // A panicked or cancelled blocking task is an enact that did
                // not happen — reported as a failure the broker can retry,
                // never propagated as a panic that would take the tick with it.
                Err(join) => {
                    return Err(EnactError::Transient(format!("admit task failed: {join}")))
                }
            }
        }

        // Trust the hash. Failure is fail-CLOSED: the binary is stored but
        // UNTRUSTED, so it cannot execute. Transient (retry the in-memory grant);
        // on retry `already` is true so admit is skipped but trust still runs.
        self.trust
            .trust_hash(&sha)
            .await
            .map_err(|e| EnactError::Transient(format!("trust failed: {e}")))?;

        // Drop the launcher (Name= is desktop-escaped; Exec= is the store path).
        self.store
            .launcher(&sha, &params.name)
            .await
            .map_err(|e| EnactError::Transient(e.to_string()))?;

        Ok(EnactOutcome {
            detail: Some(if already { "re-asserted" } else { "approved" }.into()),
        })
    }
}

/// Why building an exec.allow request failed (no request is published).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecRequestError {
    /// The client sent no `{"path": ...}` to admit.
    MissingPath,
    /// The confused-deputy guard rejected the path.
    ConfusedDeputy(ExecPathError),
    /// The source could not be inspected.
    SourceMissing,
    /// The derived name was invalid (should not happen after sanitize).
    BadName,
    /// The inspected hash was malformed.
    BadHash,
}

impl std::fmt::Display for ExecRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecRequestError::MissingPath => write!(f, "no file path in the exec.allow request"),
            ExecRequestError::ConfusedDeputy(e) => {
                let why = match e {
                    ExecPathError::Traversal => "the path contains '..'",
                    ExecPathError::OutsideManagedTree => "the file is outside your home folder",
                    ExecPathError::NotReadableByCaller => "you don't have read access to that file",
                    ExecPathError::SpecialFile => "that isn't a regular file",
                };
                write!(f, "can't approve this file: {why}")
            }
            ExecRequestError::SourceMissing => write!(f, "the file could not be read"),
            ExecRequestError::BadName => write!(f, "invalid application name"),
            ExecRequestError::BadHash => write!(f, "the file could not be hashed"),
        }
    }
}

/// The resolved exec.allow request: the params + the reqId source-path binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub params: ExecAllowParams,
    /// The guard's **resolved** path — every symlink already followed, proven
    /// beneath the caller's managed root — never the raw string the ward sent.
    /// This is what the enactor re-opens, and only through
    /// [`crate::exec_guard::open_bound_candidate`].
    pub source_path: String,
}

/// Build an `exec.allow` request: confused-deputy guard, then inspect (hash +
/// size). The hash never leaves charterd in the surfaced metadata.
pub async fn build_exec_allow_request<S: ApprovedExecStore>(
    store: &S,
    source_path: &str,
    managed_root: &str,
    caller_uid: u32,
    probe: &dyn ProbeExec,
) -> Result<ExecRequest, ExecRequestError> {
    // 03-B8 (TOCTOU): the guard hands back the OPEN candidate, not a verdict on
    // a path. Inspecting from that descriptor is what binds the hash to the
    // file that passed the guard — an `inspect(source_path)` here would walk
    // the ward's own directory tree a second time, and they are free to have
    // re-pointed a component of it since.
    let validated = validate_exec_candidate_open(source_path, managed_root, caller_uid, probe)
        .map_err(ExecRequestError::ConfusedDeputy)?;
    // What gets BOUND to the reqId is the guard's RESOLVED path, not the string
    // the ward handed us. The raw name is a request about the tree as it was:
    // every component of it belongs to the ward, and between this request and
    // the grant that answers it they are free to re-point any of them. The
    // resolved path is the one the guard proved beneath the managed root, with
    // every symlink already followed once, under charterd's own eyes — and it
    // is the only path the enactor will consent to re-open (see
    // `exec_guard::open_bound_candidate`).
    let resolved = validated
        .resolved
        .to_str()
        .ok_or(ExecRequestError::SourceMissing)?
        .to_string();
    let inspect = store
        .inspect_from_file(&validated.file, &resolved)
        .await
        .map_err(|_| ExecRequestError::SourceMissing)?;
    // The display name still comes from what the ward asked for: it is a label,
    // never a path, and it is validated + desktop-escaped downstream.
    let name = sanitize_name_from_path(source_path);
    let sha256 = Sha256Hex::from_hex(&inspect.sha256).map_err(|_| ExecRequestError::BadHash)?;
    Ok(ExecRequest {
        params: ExecAllowParams {
            name,
            sha256,
            size: inspect.size,
            origin: None,
        },
        source_path: resolved,
    })
}

/// Resolve what the live `submit_request` must hand the broker for an
/// `exec.allow`: the SERVER-inspected [`ExecAllowParams`] (as JSON) and the real
/// `source_path` to bind to the reqId. `params` is the client's request body
/// (`{"path": ...}` from `charter run`); `caller_uid` is the KERNEL-attested uid
/// of the D-Bus caller (never taken from `params`) and `managed_root` is that
/// caller's own home — together they drive the confused-deputy guard.
///
/// This is the seam the live handler was missing: it used to publish the raw
/// `{"path": ...}` with `source_path = None`, so every real `exec.allow` enact
/// died with "no source path bound to reqId". Returning the bound path here — and
/// having the handler forward it as `Some(_)` — is what makes real grants enact.
/// The returned params carry NO raw path (only name + sha256 + size); the hash
/// authority never comes from the client.
pub async fn plan_exec_allow<S: ApprovedExecStore>(
    params: &serde_json::Value,
    caller_uid: u32,
    store: &S,
    managed_root: &str,
    probe: &dyn ProbeExec,
) -> Result<(serde_json::Value, String), ExecRequestError> {
    let candidate = params
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or(ExecRequestError::MissingPath)?;
    let req = build_exec_allow_request(store, candidate, managed_root, caller_uid, probe).await?;
    let params = serde_json::to_value(&req.params).map_err(|_| ExecRequestError::BadHash)?;
    Ok((params, req.source_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_from_path_is_sanitized() {
        assert_eq!(
            sanitize_name_from_path("/home/managed/SuperTuxKart.AppImage"),
            "SuperTuxKart"
        );
        assert_eq!(sanitize_name_from_path("/home/managed/.hidden"), "app"); // empty stem -> fallback
        assert_eq!(sanitize_name_from_path("/x/wei;rd|name.bin"), "weirdname");
    }
}
