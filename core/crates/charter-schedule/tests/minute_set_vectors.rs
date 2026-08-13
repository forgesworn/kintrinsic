//! Frozen cross-stack vectors for the MinuteSet bitmap — the PWA asserts the
//! SAME file (usage_sync/minute_set_vectors.json), so any drift in the
//! base64url layout breaks loudly on both stacks.

use charter_schedule::MinuteSet;
use serde_json::Value;

fn load() -> Value {
    charter_testkit::golden::load_json("usage_sync/minute_set_vectors.json")
}

/// "minutes" is either an index array or a "range:a-b" (inclusive) string.
fn minutes_of(v: &Value) -> Vec<usize> {
    match v {
        Value::Array(a) => a.iter().map(|n| n.as_u64().unwrap() as usize).collect(),
        Value::String(s) => {
            let r = s.strip_prefix("range:").expect("range: prefix");
            let (a, b) = r.split_once('-').expect("range a-b");
            (a.parse().unwrap()..=b.parse().unwrap()).collect()
        }
        other => panic!("bad minutes spec: {other}"),
    }
}

fn set_of(minutes: &[usize]) -> MinuteSet {
    let mut m = MinuteSet::default();
    for &i in minutes {
        m.set(i);
    }
    m
}

#[test]
fn vectors_encode_and_decode_to_pinned_b64url() {
    let doc = load();
    let vectors = doc["vectors"].as_array().expect("vectors");
    assert!(!vectors.is_empty());
    for v in vectors {
        let name = v["name"].as_str().unwrap();
        let m = set_of(&minutes_of(&v["minutes"]));
        let pinned = v["b64url"].as_str().unwrap();
        assert_eq!(m.to_b64url(), pinned, "encode drift: {name}");
        assert_eq!(
            MinuteSet::from_b64url(pinned).as_ref(),
            Some(&m),
            "decode drift: {name}"
        );
    }
}

#[test]
fn union_vectors_count_overlap_once() {
    let doc = load();
    let unions = doc["unions"].as_array().expect("unions");
    assert!(!unions.is_empty());
    for u in unions {
        let name = u["name"].as_str().unwrap();
        let a = set_of(&minutes_of(&u["a"]));
        let b = set_of(&minutes_of(&u["b"]));
        let expect = u["unionCount"].as_u64().unwrap() as u32;
        assert_eq!(a.union(&b).count(), expect, "union drift: {name}");
        if let Some(pinned) = u["aB64url"].as_str() {
            assert_eq!(a.to_b64url(), pinned, "a encode drift: {name}");
        }
        if let Some(pinned) = u["bB64url"].as_str() {
            assert_eq!(b.to_b64url(), pinned, "b encode drift: {name}");
        }
    }
}

#[test]
fn invalid_strings_are_rejected() {
    let doc = load();
    let invalid = doc["invalid"].as_array().expect("invalid");
    assert!(!invalid.is_empty());
    for s in invalid {
        let s = s.as_str().unwrap();
        assert!(
            MinuteSet::from_b64url(s).is_none(),
            "accepted invalid bitmap: {s:.20}..."
        );
    }
}
