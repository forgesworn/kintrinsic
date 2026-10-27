//! Golden cross-stack vectors for the `debugging` clause. The SAME file is
//! asserted from the guardian app (TS producer). This side proves the
//! consumer: the pinned wire payload deserializes to a `GrantDebugging` and
//! `evaluate_debugging` yields the expected allowed/blocked result at each
//! given instant, and every `invalid` payload fails closed.

use charter_schedule::{evaluate_debugging, GrantDebugging};
use serde_json::Value;

#[test]
fn debugging_clause_vectors_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json("debugging/debugging_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no debugging vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let g: GrantDebugging = serde_json::from_value(v["payload"].clone())
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantDebugging: {e:?}"));

        let checks = v["expect"]["allowedAt"]
            .as_array()
            .expect("allowedAt array");
        assert!(!checks.is_empty(), "{name}: no allowedAt checks");
        for c in checks {
            let now = c["now"].as_u64().unwrap();
            assert_eq!(
                evaluate_debugging(Some(&g), now),
                c["allowed"].as_bool().unwrap(),
                "{name}: allowedAt {now}"
            );
        }
    }
}

#[test]
fn debugging_clause_invalid_payloads_fail_closed() {
    let file: Value = charter_testkit::golden::load_json("debugging/debugging_clause_vectors.json");
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        assert!(
            serde_json::from_value::<GrantDebugging>(v["payload"].clone()).is_err(),
            "{name}: must fail closed"
        );
    }
}
