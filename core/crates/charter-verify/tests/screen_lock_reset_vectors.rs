//! Hand-written cross-stack vectors for the guardian SCREEN_LOCK_RESET
//! (kind 31119). The SAME file is asserted from the guardian app (TS
//! producer). This side proves the consumer: each pinned `content` parses to
//! its `payload` and reserializes byte-identically, every `invalid` payload is
//! refused, the inner kind and tags are the ones the device acts on, and the
//! acceptance window on `issuedAt` is the pinned one, both edges inclusive.

use charter_primitives::kinds;
use charter_proto::ScreenLockResetPayload;
use charter_verify::{
    screen_lock_reset_in_window, SCREEN_LOCK_RESET_MAX_AGE_SECS, SCREEN_LOCK_RESET_MAX_AHEAD_SECS,
};
use serde_json::Value;

fn load() -> Value {
    charter_testkit::golden::load_json("screen_lock/screen_lock_reset_vectors.json")
}

#[test]
fn the_inner_event_is_the_kind_and_tags_the_device_acts_on() {
    let doc = load();
    assert_eq!(
        doc["kind"].as_u64(),
        Some(kinds::CHARTER_DEVICE_SCREEN_LOCK_RESET as u64)
    );
    let tags: Vec<Vec<String>> = serde_json::from_value(doc["tags"].clone()).unwrap();
    assert_eq!(tags, vec![kinds::marker_tag()]);
}

#[test]
fn vectors_parse_and_reserialize_byte_identically() {
    let doc = load();
    let vectors = doc["vectors"].as_array().expect("vectors");
    assert!(!vectors.is_empty());
    for v in vectors {
        let name = v["name"].as_str().unwrap();
        let content = v["content"].as_str().unwrap();
        let parsed = ScreenLockResetPayload::from_json(content)
            .unwrap_or_else(|e| panic!("{name}: refused: {e:?}"));
        assert_eq!(parsed.to_json(), content, "{name}: wire bytes drift");
        let back: Value = serde_json::from_str(&parsed.to_json()).unwrap();
        assert_eq!(back, v["payload"], "{name}: payload drift");
        assert_eq!(
            parsed.removes_screen_lock(),
            v["removesScreenLock"].as_bool().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn invalid_payloads_are_refused() {
    let doc = load();
    let invalid = doc["invalid"].as_array().expect("invalid");
    assert!(!invalid.is_empty());
    for v in invalid {
        let name = v["name"].as_str().unwrap();
        assert!(
            ScreenLockResetPayload::from_json(&v["payload"].to_string()).is_err(),
            "{name}: must be refused"
        );
    }
}

#[test]
fn the_acceptance_window_is_pinned() {
    let w = &load()["window"];
    assert_eq!(
        w["maxAgeSecs"].as_u64(),
        Some(SCREEN_LOCK_RESET_MAX_AGE_SECS)
    );
    assert_eq!(
        w["maxAheadSecs"].as_u64(),
        Some(SCREEN_LOCK_RESET_MAX_AHEAD_SECS)
    );
    for c in w["checks"].as_array().unwrap() {
        let (now, at) = (c["now"].as_u64().unwrap(), c["issuedAt"].as_u64().unwrap());
        assert_eq!(
            screen_lock_reset_in_window(at, now),
            c["inWindow"].as_bool().unwrap(),
            "issuedAt {at} at {now}"
        );
    }
}
