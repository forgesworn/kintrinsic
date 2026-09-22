//! The `exec.allow` enactor + request builder. Authority is the **sha256** in
//! the signed grant — never a path. The enactor re-hashes the COPIED bytes
//! while relocating into the root-owned store (TOCTOU close), stores under the
//! sha256 filename, validates + desktop-escapes the display name (it never
//! reaches a path), trusts the hash in fapolicyd, and drops a launcher. The
//! consumed-id was already burned at verify; a transient enact failure may only
//! retry the in-memory verified grant.

use async_trait::async_trait;

use charter_primitives::Sha256Hex;
use charter_proto::{ExecAllowParams, GrantParams, OpType};
use charter_sys::effects::{AdmitMeta, ApprovedExecStore, TrustDb};
use charter_sys::SysError;
use charter_verify::VerifiedGrant;

use crate::enactor::{EnactContext, EnactOutcome, Enactor};
use crate::error::EnactError;
use crate::exec_guard::{validate_exec_candidate_open, ExecPathError, ProbeExec};

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
pub struct ExecAllowEnactor<S: ApprovedExecStore, T: TrustDb> {
    store: S,
    trust: T,
}

impl<S: ApprovedExecStore, T: TrustDb> ExecAllowEnactor<S, T> {
    pub fn new(store: S, trust: T) -> Self {
        ExecAllowEnactor { store, trust }
    }
    pub fn store(&self) -> &S {
        &self.store
    }
    pub fn trust(&self) -> &T {
        &self.trust
    }
}

#[async_trait]
impl<S: ApprovedExecStore, T: TrustDb> Enactor for ExecAllowEnactor<S, T> {
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
            match self.store.admit(source_path, &sha, &meta).await {
                Ok(()) => {}
                Err(SysError::Conflict(m)) => return Err(EnactError::Terminal(m)),
                Err(SysError::NotFound) => {
                    return Err(EnactError::Terminal("source missing".into()))
                }
                Err(SysError::Unsupported(m)) => return Err(EnactError::Terminal(m)),
                Err(e) => return Err(EnactError::Transient(e.to_string())),
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
    let inspect = store
        .inspect_from_file(&validated.file, source_path)
        .await
        .map_err(|_| ExecRequestError::SourceMissing)?;
    let name = sanitize_name_from_path(source_path);
    let sha256 = Sha256Hex::from_hex(&inspect.sha256).map_err(|_| ExecRequestError::BadHash)?;
    Ok(ExecRequest {
        params: ExecAllowParams {
            name,
            sha256,
            size: inspect.size,
            origin: None,
        },
        source_path: source_path.to_string(),
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
