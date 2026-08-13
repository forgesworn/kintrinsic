//! The RELEASE payload (kind `CHARTER_DEVICE_RELEASE`, 31116): a parent-gated
//! unpair. The guardian signs it and gift-wraps it to the machine; the device
//! authenticates the guardian's inner signature, confirms it targets THIS
//! machine, then drops its pairing and all enforcement. Only the pinned
//! guardian's key can produce a valid one, so a child can never release their
//! own device. Content is a machine pubkey + `issuedAt` — no PII.

use serde::{Deserialize, Serialize};

use charter_primitives::PubKey;

use crate::error::ProtoError;

/// The decoded RELEASE content (JSON, `camelCase`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePayload {
    pub v: u32,
    /// The device this release targets — must equal the receiver's machine
    /// pubkey, so one device's release can never unpair another.
    pub machine: PubKey,
    /// Guardian clock (unix seconds); the device also bounds freshness.
    pub issued_at: u64,
}

impl ReleasePayload {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("release serializes")
    }

    pub fn from_json(s: &str) -> Result<ReleasePayload, ProtoError> {
        let p: ReleasePayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != 1 {
            return Err(ProtoError::BadVersion(p.v));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_roundtrips_camel_case() {
        let r = ReleasePayload {
            v: 1,
            machine: PubKey::from_bytes([0xAB; 32]),
            issued_at: 1_700_000_000,
        };
        let json = r.to_json();
        assert!(json.contains("\"machine\""));
        assert!(json.contains("\"issuedAt\""));
        assert_eq!(ReleasePayload::from_json(&json).unwrap(), r);
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut r = ReleasePayload {
            v: 2,
            machine: PubKey::from_bytes([1; 32]),
            issued_at: 1,
        };
        assert!(ReleasePayload::from_json(&r.to_json()).is_err());
        r.v = 1;
        assert!(ReleasePayload::from_json(&r.to_json()).is_ok());
    }
}
