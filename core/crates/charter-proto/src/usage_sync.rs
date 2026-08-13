//! The USAGE_SYNC payload (kind `CHARTER_DEVICE_USAGE_SYNC`, 31115): the
//! guardian-side consolidated cross-device usage view for one child, sent to
//! each device so it can enforce the POOLED budget (`spec/contract.md`
//! §USAGE_SYNC). Guardian-signed inner rumor, verified like a CLAUSE, and
//! monotonic by `ts`. Scalars carry the frozen spent-elsewhere model; the
//! optional `elsewhereMinutesToday` bitmap carries the union-rule extension
//! (simultaneous use across devices counts once). Numbers + a bitmap only —
//! no PII.

use serde::{Deserialize, Serialize};

use charter_primitives::PubKey;

use crate::error::ProtoError;

/// Current usage-sync schema version.
pub const USAGE_SYNC_VERSION: u32 = 1;

/// The decoded USAGE_SYNC content (JSON, `camelCase`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSyncPayload {
    pub v: u32,
    /// The child this view consolidates (== Signet `dependantId`).
    pub subject: PubKey,
    /// Guardian clock (unix seconds). Monotonic: a receiver stores the
    /// highest seen per subject and rejects anything not strictly newer.
    pub ts: u64,
    /// The local day this view describes (`YYYY-MM-DD`). A receiver whose
    /// current day key differs treats the whole view as rolled (contributes
    /// zero) — persist-last-known, never carry a stale day forward.
    pub day_key: String,
    /// Σ `usedTodaySecs` of the child's OTHER devices (receiver excluded, so
    /// there is no double-count in the scalar fallback).
    pub spent_elsewhere_today_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_key: Option<String>,
    /// Σ week usage of the OTHER devices, when the aggregator has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_elsewhere_week_secs: Option<u64>,
    /// Union-rule extension: the union of the OTHER devices' active minutes
    /// today as a MinuteSet base64url string (240 chars). When both sides
    /// have bitmaps the pooled day is `|own ∪ elsewhere|` minutes; absent →
    /// scalar fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere_minutes_today: Option<String>,
}

impl UsageSyncPayload {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("usage-sync serializes")
    }

    pub fn from_json(s: &str) -> Result<UsageSyncPayload, ProtoError> {
        let p: UsageSyncPayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != USAGE_SYNC_VERSION {
            return Err(ProtoError::BadVersion(p.v));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> UsageSyncPayload {
        UsageSyncPayload {
            v: 1,
            subject: PubKey::from_bytes([0xCD; 32]),
            ts: 1_782_734_400,
            day_key: "2026-06-29".to_string(),
            spent_elsewhere_today_secs: 1800,
            week_key: None,
            spent_elsewhere_week_secs: None,
            elsewhere_minutes_today: None,
        }
    }

    #[test]
    fn roundtrips_camel_case() {
        let p = UsageSyncPayload {
            week_key: Some("2026-06-29".to_string()),
            spent_elsewhere_week_secs: Some(7200),
            elsewhere_minutes_today: Some("A".repeat(240)),
            ..payload()
        };
        let json = p.to_json();
        for key in [
            "\"dayKey\"",
            "\"spentElsewhereTodaySecs\"",
            "\"weekKey\"",
            "\"spentElsewhereWeekSecs\"",
            "\"elsewhereMinutesToday\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        assert_eq!(UsageSyncPayload::from_json(&json).unwrap(), p);
    }

    #[test]
    fn optional_fields_omitted_when_absent() {
        let json = payload().to_json();
        assert!(!json.contains("weekKey"));
        assert!(!json.contains("spentElsewhereWeekSecs"));
        assert!(!json.contains("elsewhereMinutesToday"));
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut p = payload();
        p.v = 2;
        assert!(matches!(
            UsageSyncPayload::from_json(&p.to_json()),
            Err(ProtoError::BadVersion(2))
        ));
    }

    #[test]
    fn unknown_fields_tolerated_for_forward_compat() {
        let mut v: serde_json::Value = serde_json::from_str(&payload().to_json()).unwrap();
        v["futureField"] = serde_json::json!(true);
        assert!(UsageSyncPayload::from_json(&v.to_string()).is_ok());
    }
}
