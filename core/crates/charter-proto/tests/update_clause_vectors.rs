//! Golden cross-stack vectors for the `update` clause (#44). The SAME file is
//! asserted from MyCharter (TS producer: updateToGrant). This side proves the
//! consumer: every pinned wire payload deserializes to an `UpdateAppBody` and
//! validates, and every `invalid` payload fails closed.

use charter_proto::UpdateAppBody;
use serde_json::Value;

#[test]
fn update_clause_vectors_parse_and_validate() {
    let file: Value = charter_testkit::golden::load_json("update/update_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no update vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let body: UpdateAppBody = serde_json::from_value(v["payload"].clone())
            .unwrap_or_else(|e| panic!("{name}: payload must deserialize: {e}"));
        body.validate()
            .unwrap_or_else(|e| panic!("{name}: payload must validate: {e:?}"));
        // Round-trip: re-serializing must reproduce the pinned payload exactly
        // (field names and values — the producer/consumer byte contract).
        let back = serde_json::to_value(&body).expect("serialize");
        assert_eq!(back, v["payload"], "{name}: round-trip drift");
    }
}

#[test]
fn update_clause_invalid_payloads_fail_closed() {
    let file: Value = charter_testkit::golden::load_json("update/update_clause_vectors.json");
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        let parsed: Result<UpdateAppBody, _> = serde_json::from_value(v["payload"].clone());
        let rejected = match parsed {
            Err(_) => true,
            Ok(body) => body.validate().is_err(),
        };
        assert!(rejected, "{name}: must fail closed");
    }
}
