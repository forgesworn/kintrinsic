//! Golden cross-stack vectors for the `tethering` clause. The SAME file is
//! asserted from MyCharter (TS producer: tetheringToGrant). This side proves the
//! consumer: the pinned wire payload deserializes to a `GrantTethering` and
//! `evaluate_tethering` yields the expected mode at the given instants, and
//! every `invalid` payload fails closed.

use charter_schedule::{evaluate_tethering, GrantTethering, TetherMode};
use serde_json::Value;

fn mode_str(m: TetherMode) -> &'static str {
    match m {
        TetherMode::Blocked => "blocked",
        TetherMode::Raw => "raw",
        TetherMode::Filtered => "filtered",
    }
}

#[test]
fn tethering_clause_vectors_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json("tethering/tethering_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no tethering vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let g: GrantTethering = serde_json::from_value(v["payload"].clone())
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantTethering: {e:?}"));

        let at = &v["expect"]["modeAt"];
        let now = at["now"].as_u64().unwrap();
        assert_eq!(
            mode_str(evaluate_tethering(Some(&g), now)),
            at["mode"].as_str().unwrap(),
            "{name}: modeAt"
        );

        if let Some(exp) = v["expect"].get("modeAtExpiry") {
            let now = exp["now"].as_u64().unwrap();
            assert_eq!(
                mode_str(evaluate_tethering(Some(&g), now)),
                exp["mode"].as_str().unwrap(),
                "{name}: modeAtExpiry"
            );
        }
    }
}

#[test]
fn tethering_clause_invalid_payloads_fail_closed() {
    let file: Value = charter_testkit::golden::load_json("tethering/tethering_clause_vectors.json");
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        assert!(
            serde_json::from_value::<GrantTethering>(v["payload"].clone()).is_err(),
            "{name}: must fail closed"
        );
    }
}
