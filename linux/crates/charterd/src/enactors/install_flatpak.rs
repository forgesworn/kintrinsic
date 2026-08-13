//! The `install.flatpak` enactor + request builder. The enactor acts on the
//! ref **inside the signed grant**, re-parses it through `FlatpakRef::parse`
//! before any `FlatpakOps` call (injection guard), and installs idempotently.

use async_trait::async_trait;

use charter_proto::{FlatpakRef, GrantParams, OpType};
use charter_sys::effects::FlatpakOps;
use charter_verify::VerifiedGrant;

use crate::enactor::{EnactContext, EnactOutcome, Enactor};
use crate::error::EnactError;

/// The charterd-tightened overrides applied to every system install — an
/// approved game can't become a general exec/escape path. Pure; asserted by a
/// test (the real impl passes these as argv elements).
pub fn tightened_override_args() -> Vec<String> {
    vec![
        "--nofilesystem=host".to_string(),
        "--no-talk-name=org.freedesktop.Flatpak".to_string(),
    ]
}

/// Enacts `install.flatpak` behind `FlatpakOps`.
pub struct InstallFlatpakEnactor<F: FlatpakOps> {
    flatpak: F,
}

impl<F: FlatpakOps> InstallFlatpakEnactor<F> {
    pub fn new(flatpak: F) -> Self {
        InstallFlatpakEnactor { flatpak }
    }

    /// Borrow the FlatpakOps (test inspection).
    pub fn flatpak(&self) -> &F {
        &self.flatpak
    }
}

#[async_trait]
impl<F: FlatpakOps> Enactor for InstallFlatpakEnactor<F> {
    fn op(&self) -> OpType {
        OpType::InstallFlatpak
    }

    async fn enact(
        &self,
        grant: &VerifiedGrant,
        _ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        let params = match grant.allow_params() {
            Some(GrantParams::InstallFlatpak(p)) => p,
            _ => return Err(EnactError::Terminal("not an install.flatpak allow".into())),
        };
        // Injection guard: re-parse the granted ref BEFORE touching FlatpakOps.
        let reference = FlatpakRef::parse(&params.reference)
            .map_err(|_| EnactError::Terminal("invalid flatpak ref".into()))?;
        // `remote` is typed `Remote::Flathub` by deserialization — nothing else
        // could have parsed into the grant params.

        // Idempotent: an already-installed ref is a no-op (no FlatpakOps install).
        let installed = self
            .flatpak
            .is_installed_system(reference.as_str())
            .await
            .map_err(|e| EnactError::Transient(e.to_string()))?;
        if installed {
            return Ok(EnactOutcome {
                detail: Some("already installed".into()),
            });
        }

        self.flatpak
            .install_system(reference.as_str())
            .await
            .map_err(|e| EnactError::Transient(e.to_string()))?;
        Ok(EnactOutcome {
            detail: Some(format!("installed {}", reference.as_str())),
        })
    }
}

/// Why building an install request failed (no request is published).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestBuildError {
    InvalidRef,
    NotFound,
    Offline,
}

/// The resolved install request: the params to broker + the app's requested
/// permissions surfaced to the guardian (advisory, non-authoritative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRequest {
    pub params: serde_json::Value,
    pub app_name: String,
    pub permissions: Vec<String>,
}

/// Resolve a Flathub ref into a request (with the permission summary). Unknown
/// ref -> NotFound; offline -> Offline; injection -> InvalidRef. No request on
/// error (fail-closed).
pub async fn build_install_request<F: FlatpakOps>(
    flatpak: &F,
    reference: &str,
) -> Result<InstallRequest, RequestBuildError> {
    let r = FlatpakRef::parse(reference).map_err(|_| RequestBuildError::InvalidRef)?;
    let info = flatpak.resolve(r.as_str()).await.map_err(|e| match e {
        charter_sys::SysError::NotFound => RequestBuildError::NotFound,
        _ => RequestBuildError::Offline,
    })?;
    Ok(InstallRequest {
        params: serde_json::json!({"ref": info.reference, "remote": "flathub"}),
        app_name: info.name,
        permissions: info.permissions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tightened_overrides_drop_host_fs_and_flatpak_talk() {
        let args = tightened_override_args();
        assert!(args.iter().any(|a| a.contains("nofilesystem=host")));
        assert!(args
            .iter()
            .any(|a| a.contains("no-talk-name=org.freedesktop.Flatpak")));
    }
}
