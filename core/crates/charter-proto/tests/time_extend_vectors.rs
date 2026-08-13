//! The frozen `time.extend` wire shape: u16 minutes, 1440 cap, grant echoes
//! `limitHit`.

use charter_proto::{LimitHit, TimeExtendGrantParams, TimeExtendRequestParams, MAX_EXTEND_MINUTES};

#[derive(serde::Deserialize)]
struct Vectors {
    request: TimeExtendRequestParams,
    grant_full: TimeExtendGrantParams,
    grant_less: TimeExtendGrantParams,
    grant_zero_noop: TimeExtendGrantParams,
    invalid_request_zero: TimeExtendRequestParams,
    invalid_grant_over_max: TimeExtendGrantParams,
}

fn vectors() -> Vectors {
    charter_testkit::golden::load_json("proto/time_extend.json")
}

#[test]
fn frozen_shape_and_validation() {
    assert_eq!(MAX_EXTEND_MINUTES, 1440);
    let v = vectors();

    // Request validates; the grant echoes limitHit (exact params-echo).
    assert!(v.request.validate().is_ok());
    assert_eq!(v.request.limit_hit, LimitHit::Budget);
    assert!(v.grant_full.validate().is_ok());
    assert_eq!(v.grant_full.limit_hit, LimitHit::Budget);
    assert_eq!(v.grant_full.minutes_granted, 30);

    // Guardian may grant less, and switch nothing else (limitHit preserved).
    assert_eq!(v.grant_less.minutes_granted, 15);
    assert_eq!(v.grant_less.limit_hit, LimitHit::Schedule);

    // Zero is a valid (no-op) grant.
    assert!(v.grant_zero_noop.validate().is_ok());
    assert_eq!(v.grant_zero_noop.minutes_granted, 0);

    // Out-of-range cases are rejected.
    assert!(v.invalid_request_zero.validate().is_err());
    assert!(v.invalid_grant_over_max.validate().is_err());
}

#[test]
fn grant_roundtrips_echoing_limit_hit() {
    let g = TimeExtendGrantParams {
        minutes_granted: 45,
        limit_hit: LimitHit::Schedule,
        bucket_id: None,
    };
    let json = serde_json::to_string(&g).unwrap();
    assert!(json.contains("\"limitHit\":\"schedule\""));
    assert!(json.contains("\"minutesGranted\":45"));
    let back: TimeExtendGrantParams = serde_json::from_str(&json).unwrap();
    assert_eq!(g, back);
}
