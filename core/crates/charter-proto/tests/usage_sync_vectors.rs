//! Frozen cross-stack USAGE_SYNC payload vectors — the PWA asserts the SAME
//! file, so payload drift breaks loudly on both stacks.

use charter_proto::UsageSyncPayload;
use serde_json::Value;

fn load() -> Value {
    charter_testkit::golden::load_json("usage_sync/usage_sync_vectors.json")
}

#[test]
fn vectors_parse_and_reserialize_losslessly() {
    let doc = load();
    let vectors = doc["vectors"].as_array().expect("vectors");
    assert!(!vectors.is_empty());
    for v in vectors {
        let name = v["name"].as_str().unwrap();
        let pinned = &v["payload"];
        let parsed = UsageSyncPayload::from_json(&pinned.to_string())
            .unwrap_or_else(|e| panic!("vector {name} failed to parse: {e:?}"));
        let back: Value = serde_json::from_str(&parsed.to_json()).unwrap();
        assert_eq!(&back, pinned, "reserialize drift: {name}");
    }
}

#[test]
fn invalid_payloads_fail_closed() {
    let doc = load();
    let invalid = doc["invalid"].as_array().expect("invalid");
    assert!(!invalid.is_empty());
    for v in invalid {
        let name = v["name"].as_str().unwrap();
        assert!(
            UsageSyncPayload::from_json(&v["payload"].to_string()).is_err(),
            "invalid vector accepted: {name}"
        );
    }
}
