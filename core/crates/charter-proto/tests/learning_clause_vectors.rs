//! Golden cross-stack vectors for the `learning` clause. The SAME file is
//! asserted from MyCharter (TS producer). This side proves the consumer: the
//! pinned wire payload deserializes to a `GrantLearning` with the apps/cap/
//! paused the vector expects, and every `invalid` payload fails closed.

use charter_proto::GrantLearning;
use serde_json::Value;

#[test]
fn learning_clause_vectors_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json("learning/learning_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no learning vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let payload = &v["payload"];
        let expect = &v["expect"];

        let g = GrantLearning::from_value(payload)
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantLearning: {e:?}"));

        assert_eq!(
            g.is_paused(),
            expect["paused"].as_bool().unwrap(),
            "{name}: paused"
        );
        let want_cap = expect["capMinutes"].as_u64().map(|c| c as u32);
        assert_eq!(g.cap_minutes, want_cap, "{name}: capMinutes");
        assert_eq!(
            g.apps.len() as u64,
            expect["appCount"].as_u64().unwrap(),
            "{name}: appCount"
        );
    }
}

#[test]
fn learning_clause_invalid_payloads_fail_closed() {
    let file: Value = charter_testkit::golden::load_json("learning/learning_clause_vectors.json");
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        assert!(
            GrantLearning::from_value(&v["payload"]).is_err(),
            "{name}: must fail closed"
        );
    }
}
