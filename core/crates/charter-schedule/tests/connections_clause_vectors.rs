//! Golden cross-stack vectors for the `connections` clause. The SAME file is
//! asserted from the guardian app (TS producer: connectionsToGrant). This side
//! proves the consumer: each pinned payload deserializes to a
//! `GrantConnections` and evaluates to the expected posture, and every
//! `invalid` payload fails to parse (the warden then enforces the careful
//! tier).

use charter_schedule::{evaluate_connections, GrantConnections};
use serde_json::Value;

#[test]
fn connections_clause_vectors_parse_and_match() {
    let file: Value =
        charter_testkit::golden::load_json("connections/connections_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no connections vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let g: GrantConnections = serde_json::from_value(v["payload"].clone())
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantConnections: {e:?}"));
        let p = evaluate_connections(Some(&g));
        assert_eq!(
            p.wifi.as_str(),
            v["expect"]["wifi"].as_str().unwrap(),
            "{name}: wifi"
        );
        assert_eq!(
            p.bluetooth.as_str(),
            v["expect"]["bluetooth"].as_str().unwrap(),
            "{name}: bluetooth"
        );
    }
}

#[test]
fn connections_clause_invalid_payloads_fail_closed() {
    let file: Value =
        charter_testkit::golden::load_json("connections/connections_clause_vectors.json");
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        assert!(
            serde_json::from_value::<GrantConnections>(v["payload"].clone()).is_err(),
            "{name}: must fail closed"
        );
    }
}
