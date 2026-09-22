//! The `tethering` clause body + evaluator: the guardian's standing posture for
//! the ward's hotspot/tethering. Default (no clause) is BLOCKED — the hotspot
//! hands tethered clients raw upstream internet (kernel-forwarded around any
//! on-device filter), so it opens only by explicit grant, optionally time-boxed.

use serde::{Deserialize, Serialize};

/// The frozen `tethering` clause body version.
pub const TETHERING_VERSION: u32 = 1;

/// What the guardian allows: nothing, the raw system hotspot (tethered clients
/// get UNFILTERED internet — the UI must say so), or the Charter-filtered
/// hotspot (guests ride the on-phone filtering proxy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TetherAllow {
    None,
    Raw,
    Filtered,
}

/// The `tethering` clause body. Replace-the-state (one clause = the whole
/// posture), riding the per-kind monotonic `issuedAt` rollback protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantTethering {
    pub v: u32,
    pub issued_at: u64,
    pub allow: TetherAllow,
    /// Unix seconds when the allowance ends (the grant ends AT this instant).
    /// Absent = until revoked by a superseding clause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

/// What enforcement must do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TetherMode {
    /// Everything locked (`DISALLOW_CONFIG_TETHERING`). The default.
    Blocked,
    /// System tethering available — tethered clients are unfiltered.
    Raw,
    /// Charter Hotspot session: system Wi-Fi hotspot stays locked, the app
    /// hosts its own fail-closed AP + filtering proxy.
    Filtered,
}

/// Level-triggered per-tick evaluation: no clause, an explicit `none`, an
/// expired window, or a body version this build does not implement all mean
/// Blocked — fail-safe in every direction.
///
/// The version check is why [`TETHERING_VERSION`] exists, and the fail
/// direction here is the worst one in the product: the arm that survives an
/// unrecognised body is `Raw`, which hands tethered clients unfiltered
/// upstream internet, kernel-forwarded around every on-device filter
/// (`spec/contract.md` §tethering). Serde drops unknown fields, so a `v: 2`
/// that (say) added a required scope would otherwise evaluate to "raw,
/// unbounded".
pub fn evaluate_tethering(grant: Option<&GrantTethering>, now: u64) -> TetherMode {
    let Some(g) = grant else {
        return TetherMode::Blocked;
    };
    if g.v > TETHERING_VERSION {
        return TetherMode::Blocked;
    }
    if let Some(until) = g.until {
        if now >= until {
            return TetherMode::Blocked;
        }
    }
    match g.allow {
        TetherAllow::None => TetherMode::Blocked,
        TetherAllow::Raw => TetherMode::Raw,
        TetherAllow::Filtered => TetherMode::Filtered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_clause_is_blocked() {
        assert_eq!(evaluate_tethering(None, 1_700_000_000), TetherMode::Blocked);
    }

    #[test]
    fn allow_none_is_blocked() {
        let g = GrantTethering {
            v: 1,
            issued_at: 1,
            allow: TetherAllow::None,
            until: None,
        };
        assert_eq!(
            evaluate_tethering(Some(&g), 1_700_000_000),
            TetherMode::Blocked
        );
    }

    #[test]
    fn raw_inside_window_is_raw_and_expires_at_until() {
        let g = GrantTethering {
            v: 1,
            issued_at: 1,
            allow: TetherAllow::Raw,
            until: Some(1_700_003_600),
        };
        assert_eq!(evaluate_tethering(Some(&g), 1_700_000_000), TetherMode::Raw);
        // The boundary itself is already expired — a grant "until T" ends AT T.
        assert_eq!(
            evaluate_tethering(Some(&g), 1_700_003_600),
            TetherMode::Blocked
        );
        assert_eq!(
            evaluate_tethering(Some(&g), 1_700_003_601),
            TetherMode::Blocked
        );
    }

    #[test]
    fn filtered_inside_window_is_filtered() {
        let g = GrantTethering {
            v: 1,
            issued_at: 1,
            allow: TetherAllow::Filtered,
            until: Some(1_700_003_600),
        };
        assert_eq!(
            evaluate_tethering(Some(&g), 1_700_000_000),
            TetherMode::Filtered
        );
    }

    #[test]
    fn until_absent_means_until_revoked() {
        let g = GrantTethering {
            v: 1,
            issued_at: 1,
            allow: TetherAllow::Raw,
            until: None,
        };
        assert_eq!(evaluate_tethering(Some(&g), u64::MAX), TetherMode::Raw);
    }

    /// G4: a future body version is BLOCKED, not read as v1. `raw` is the arm
    /// that would otherwise survive an unrecognised body, and it is the one
    /// that hands out unfiltered internet.
    #[test]
    fn a_future_body_version_is_blocked() {
        for allow in [TetherAllow::Raw, TetherAllow::Filtered, TetherAllow::None] {
            let g = GrantTethering {
                v: TETHERING_VERSION + 1,
                issued_at: 1,
                allow,
                until: None,
            };
            assert_eq!(
                evaluate_tethering(Some(&g), 1_700_000_000),
                TetherMode::Blocked,
                "v{} allow {allow:?}",
                g.v
            );
        }
        // …and the version this build implements still evaluates normally.
        let ok = GrantTethering {
            v: TETHERING_VERSION,
            issued_at: 1,
            allow: TetherAllow::Raw,
            until: None,
        };
        assert_eq!(
            evaluate_tethering(Some(&ok), 1_700_000_000),
            TetherMode::Raw
        );
    }

    #[test]
    fn parses_wire_json_camel_case() {
        let g: GrantTethering = serde_json::from_str(
            r#"{"v":1,"issuedAt":1700000000,"allow":"filtered","until":1700003600}"#,
        )
        .unwrap();
        assert_eq!(g.allow, TetherAllow::Filtered);
        assert_eq!(g.until, Some(1_700_003_600));
        assert_eq!(g.issued_at, 1_700_000_000);
    }

    #[test]
    fn parses_wire_json_without_until() {
        let g: GrantTethering =
            serde_json::from_str(r#"{"v":1,"issuedAt":1700000000,"allow":"raw"}"#).unwrap();
        assert_eq!(g.until, None);
    }

    #[test]
    fn serializes_without_until_when_none() {
        let g = GrantTethering {
            v: 1,
            issued_at: 5,
            allow: TetherAllow::None,
            until: None,
        };
        let json = serde_json::to_string(&g).unwrap();
        assert!(!json.contains("until"), "got: {json}");
        assert!(json.contains("\"allow\":\"none\""), "got: {json}");
    }
}
