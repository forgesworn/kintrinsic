//! Golden cross-stack vectors for the per-app (`apps`) clause. The SAME file is
//! asserted from MyCharter (TS producer — `appsToGrant`). This side proves the
//! consumer: the pinned wire payload deserializes to a `GrantApps` with the
//! posture/lists/paused the vector expects. Drift on either side fails here.

use charter_proto::{AppPosture, GrantApps};
use serde_json::Value;

#[test]
fn apps_clause_vectors_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json("apps/apps_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no apps vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let payload = &v["payload"];

        let g = GrantApps::from_value(payload)
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantApps: {e:?}"));

        let want_posture = match payload["posture"].as_str().unwrap() {
            "allowlist" => AppPosture::Allowlist,
            _ => AppPosture::Blocklist,
        };
        assert_eq!(g.posture, want_posture, "{name}: posture");
        assert_eq!(
            g.is_paused(),
            payload["paused"].as_bool().unwrap_or(false),
            "{name}: paused"
        );

        let want_blocked: Vec<String> = payload["blocked"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        assert_eq!(g.blocked, want_blocked, "{name}: blocked");

        let want_allowed: Vec<String> = payload["allowed"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        assert_eq!(g.allowed, want_allowed, "{name}: allowed");

        // `hidden` ("remove from device") is additive and independent of
        // posture/paused — a vector that carries it pins that it survives.
        let want_hidden: Vec<String> = payload["hidden"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();
        assert_eq!(g.hidden, want_hidden, "{name}: hidden");

        // Holds carry an ABSOLUTE expiry, so a vector that has one also pins what
        // the device must be enforcing at a stated instant — the whole point of
        // the field is that the two sides agree about when it ends.
        let want_holds = payload["holds"].as_array().cloned().unwrap_or_default();
        assert_eq!(g.holds.len(), want_holds.len(), "{name}: hold count");
        for (got, want) in g.holds.iter().zip(want_holds.iter()) {
            assert_eq!(got.pkg, want["pkg"].as_str().unwrap(), "{name}: hold pkg");
            assert_eq!(
                got.until_unix,
                want["untilUnix"].as_u64().unwrap(),
                "{name}: hold untilUnix"
            );
            let want_state = match want["state"].as_str().unwrap() {
                "allowed" => charter_proto::HoldState::Allowed,
                other => {
                    assert_eq!(other, "blocked", "{name}: unknown hold state");
                    charter_proto::HoldState::Blocked
                }
            };
            assert_eq!(got.state, want_state, "{name}: hold state");
        }

        if let Some(at) = v["effectiveAt"].as_u64() {
            let eff = g.effective_at(at);
            let want_blocked: Vec<String> = strings(&v["effectiveBlocked"]);
            let want_allowed: Vec<String> = strings(&v["effectiveAllowed"]);
            assert_eq!(eff.blocked, want_blocked, "{name}: effective blocked");
            assert_eq!(eff.allowed, want_allowed, "{name}: effective allowed");
            assert!(eff.holds.is_empty(), "{name}: resolved clause keeps holds");
        }
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}
