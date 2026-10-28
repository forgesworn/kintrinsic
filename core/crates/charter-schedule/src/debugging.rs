//! The `debugging` clause body + evaluator: the guardian's standing posture for
//! the ward device's USB/wireless debugging (adb). Default (no clause) is
//! BLOCKED — adb can force-stop the warden, revoke the usage-stats app-op and
//! read the ward's data, so it opens only by explicit grant, optionally
//! time-boxed.

use serde::{Deserialize, Serialize};

/// The frozen `debugging` clause body version.
pub const DEBUGGING_VERSION: u32 = 1;

/// The `debugging` clause body. Replace-the-state (one clause = the whole
/// posture), riding the per-kind monotonic `issuedAt` rollback protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantDebugging {
    pub v: u32,
    pub issued_at: u64,
    /// Whether debugging is allowed. `false` is the blocked posture.
    pub allow: bool,
    /// Unix seconds when the allowance ends (the grant ends AT this instant).
    /// Absent = until revoked by a superseding clause. Meaningless when
    /// `allow` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<u64>,
}

/// Level-triggered per-tick evaluation: `true` means debugging is allowed right
/// now. No clause, `allow: false`, an expired window, or any body version other
/// than the one this build implements all mean blocked — fail-safe in every
/// direction.
///
/// The version check is why [`DEBUGGING_VERSION`] exists: serde drops unknown
/// fields, so a `v: 2` that (say) added a required scope would otherwise read
/// as "allowed, unbounded". It is an exact match, not a ceiling: `v: 0` was
/// never a debugging body either, so it is not read as v1.
pub fn evaluate_debugging(grant: Option<&GrantDebugging>, now: u64) -> bool {
    let Some(g) = grant else {
        return false;
    };
    if g.v != DEBUGGING_VERSION || !g.allow {
        return false;
    }
    match g.until {
        Some(until) => now < until,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(allow: bool, until: Option<u64>) -> GrantDebugging {
        GrantDebugging {
            v: 1,
            issued_at: 1,
            allow,
            until,
        }
    }

    #[test]
    fn no_clause_is_blocked() {
        assert!(!evaluate_debugging(None, 1_700_000_000));
    }

    #[test]
    fn allow_false_is_blocked() {
        assert!(!evaluate_debugging(
            Some(&grant(false, None)),
            1_700_000_000
        ));
    }

    #[test]
    fn allow_false_ignores_a_stray_until() {
        let g = grant(false, Some(1_700_003_600));
        assert!(!evaluate_debugging(Some(&g), 1_700_000_000));
    }

    #[test]
    fn allowed_inside_window_and_expires_at_until() {
        let g = grant(true, Some(1_700_003_600));
        assert!(evaluate_debugging(Some(&g), 1_700_000_000));
        assert!(evaluate_debugging(Some(&g), 1_700_003_599));
        // The boundary itself is already expired — a grant "until T" ends AT T.
        assert!(!evaluate_debugging(Some(&g), 1_700_003_600));
        assert!(!evaluate_debugging(Some(&g), 1_700_003_601));
    }

    #[test]
    fn until_absent_means_until_revoked() {
        assert!(evaluate_debugging(Some(&grant(true, None)), u64::MAX));
    }

    /// A future body version is BLOCKED, not read as v1.
    #[test]
    fn a_future_body_version_is_blocked() {
        let g = GrantDebugging {
            v: DEBUGGING_VERSION + 1,
            issued_at: 1,
            allow: true,
            until: None,
        };
        assert!(!evaluate_debugging(Some(&g), 1_700_000_000));
        // …and the version this build implements still evaluates normally.
        let ok = GrantDebugging {
            v: DEBUGGING_VERSION,
            ..g
        };
        assert!(evaluate_debugging(Some(&ok), 1_700_000_000));
    }

    /// `v: 0` predates the frozen v1 body: blocked, not read as v1.
    #[test]
    fn a_zero_body_version_is_blocked() {
        let g = GrantDebugging {
            v: 0,
            issued_at: 1,
            allow: true,
            until: None,
        };
        assert!(!evaluate_debugging(Some(&g), 1_700_000_000));
    }

    #[test]
    fn parses_wire_json_camel_case() {
        let g: GrantDebugging = serde_json::from_str(
            r#"{"v":1,"issuedAt":1700000000,"allow":true,"until":1700003600}"#,
        )
        .unwrap();
        assert!(g.allow);
        assert_eq!(g.until, Some(1_700_003_600));
        assert_eq!(g.issued_at, 1_700_000_000);
    }

    #[test]
    fn parses_wire_json_without_until() {
        let g: GrantDebugging =
            serde_json::from_str(r#"{"v":1,"issuedAt":1700000000,"allow":false}"#).unwrap();
        assert!(!g.allow);
        assert_eq!(g.until, None);
    }

    #[test]
    fn serializes_without_until_when_none() {
        let json = serde_json::to_string(&grant(false, None)).unwrap();
        assert!(!json.contains("until"), "got: {json}");
        assert!(json.contains("\"allow\":false"), "got: {json}");
    }
}
