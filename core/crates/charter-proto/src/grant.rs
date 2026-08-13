//! The GRANT payload — the content of a `CHARTER_DEVICE_GRANT` (kind 31112)
//! NIP-01 event signed by the guardian. The grant echoes the exact approved
//! params; the enactor acts on *these* params, never the request's.

use serde::{Deserialize, Serialize};

use charter_primitives::{Nonce, ReqId};

use crate::error::ProtoError;
use crate::op::{Decision, OpType};
use crate::params::GrantParams;

/// Current grant schema version.
pub const GRANT_VERSION: u32 = 1;

/// The decoded GRANT content. `params` is kept as a raw value and parsed
/// per-op via [`GrantPayload::grant_params`] so each enactor owns its shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantPayload {
    pub v: u32,
    pub op: OpType,
    pub req_id: ReqId,
    pub nonce: Nonce,
    pub decision: Decision,
    pub ts: u64,
    pub exp: u64,
    pub params: serde_json::Value,
}

impl GrantPayload {
    /// Parse a GRANT payload from JSON content bytes.
    pub fn from_json(s: &str) -> Result<GrantPayload, ProtoError> {
        let p: GrantPayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != GRANT_VERSION {
            return Err(ProtoError::BadVersion(p.v));
        }
        Ok(p)
    }

    /// Parse the op-specific params (only meaningful for an `allow` grant).
    pub fn grant_params(&self) -> Result<GrantParams, ProtoError> {
        GrantParams::parse(self.op, &self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Remote;

    #[test]
    fn parses_install_grant_and_params() {
        let json = r#"{"v":1,"op":"install.flatpak","reqId":"1111111111111111111111111111111111111111111111111111111111111111","nonce":"2222222222222222222222222222222222222222222222222222222222222222","decision":"allow","ts":1700000000,"exp":1700003600,"params":{"ref":"org.videolan.VLC","remote":"flathub"}}"#;
        let g = GrantPayload::from_json(json).unwrap();
        assert_eq!(g.op, OpType::InstallFlatpak);
        assert_eq!(g.decision, Decision::Allow);
        match g.grant_params().unwrap() {
            GrantParams::InstallFlatpak(p) => {
                assert_eq!(p.reference, "org.videolan.VLC");
                assert_eq!(p.remote, Remote::Flathub);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_unknown_remote_fail_closed() {
        let json = r#"{"v":1,"op":"install.flatpak","reqId":"1111111111111111111111111111111111111111111111111111111111111111","nonce":"2222222222222222222222222222222222222222222222222222222222222222","decision":"allow","ts":1,"exp":2,"params":{"ref":"x","remote":"sketchy"}}"#;
        let g = GrantPayload::from_json(json).unwrap();
        assert!(g.grant_params().is_err());
    }

    #[test]
    fn rejects_wrong_version() {
        let json = r#"{"v":2,"op":"time.extend","reqId":"1111111111111111111111111111111111111111111111111111111111111111","nonce":"2222222222222222222222222222222222222222222222222222222222222222","decision":"allow","ts":1,"exp":2,"params":{}}"#;
        assert_eq!(
            GrantPayload::from_json(json),
            Err(ProtoError::BadVersion(2))
        );
    }
}
