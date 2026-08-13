//! The REQUEST payload — the content of a `CHARTER_DEVICE_REQUEST` (kind 31111)
//! rumor the machine publishes to the guardian.

use serde::{Deserialize, Serialize};

use charter_primitives::{Nonce, PubKey, ReqId};

use crate::error::ProtoError;
use crate::op::OpType;

/// Current request schema version.
pub const REQUEST_VERSION: u32 = 1;

/// The decoded REQUEST content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPayload {
    pub v: u32,
    pub op: OpType,
    pub req_id: ReqId,
    pub nonce: Nonce,
    /// The managed user's pubkey ("who is asking").
    pub subject: PubKey,
    /// The machine's pubkey.
    pub machine: PubKey,
    pub ts: u64,
    pub params: serde_json::Value,
}

impl RequestPayload {
    /// Serialize to compact JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("request serializes")
    }

    /// Parse from JSON content bytes.
    pub fn from_json(s: &str) -> Result<RequestPayload, ProtoError> {
        let p: RequestPayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != REQUEST_VERSION {
            return Err(ProtoError::BadVersion(p.v));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips() {
        let r = RequestPayload {
            v: 1,
            op: OpType::TimeExtend,
            req_id: ReqId::from_bytes([1; 32]),
            nonce: Nonce::from_bytes([2; 32]),
            subject: PubKey::from_bytes([3; 32]),
            machine: PubKey::from_bytes([4; 32]),
            ts: 1700000000,
            params: serde_json::json!({"minutesRequested": 30, "limitHit": "budget"}),
        };
        let json = r.to_json();
        let back = RequestPayload::from_json(&json).unwrap();
        assert_eq!(r, back);
    }
}
