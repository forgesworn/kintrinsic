//! pin.json: scrypt derivation, record validity and backoff.
mod common;

use charter_guardians::event::decode_hex_vec;
use charter_guardians::{pin_backoff_seconds, pin_hash, validate_pin_record, verify_pin};

#[test]
fn scrypt_vectors() {
    let v = common::load("pin.json");
    assert_eq!(v["scrypt"]["N"], 32768);
    assert_eq!(v["scrypt"]["r"], 8);
    assert_eq!(v["scrypt"]["p"], 1);
    assert_eq!(v["scrypt"]["dkLen"], 32);
    let vectors = v["vectors"].as_array().unwrap();
    assert!(!vectors.is_empty());
    for (n, t) in vectors.iter().enumerate() {
        let pin = common::s(t, "pin");
        let salt = decode_hex_vec(common::s(t, "salt")).unwrap();
        let hash = pin_hash(pin, &salt);
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, common::s(t, "hash"), "pin {pin}");
        let rec = validate_pin_record(&t["record"]).expect("record valid");
        // scrypt is slow unoptimised: check verify_pin on the first vector only.
        if n == 0 {
            assert!(verify_pin(&rec, pin), "pin {pin} verifies");
            assert!(!verify_pin(&rec, "999999"), "a wrong pin fails");
        }
    }
}

#[test]
fn record_validity() {
    let v = common::load("pin.json");
    for t in v["records"].as_array().unwrap() {
        let want = t["valid"].as_bool().unwrap();
        assert_eq!(
            validate_pin_record(&t["record"]).is_some(),
            want,
            "{}",
            common::s(t, "name")
        );
    }
}

#[test]
fn backoff() {
    let v = common::load("pin.json");
    for t in v["backoff"].as_array().unwrap() {
        let f = t["failures"].as_u64().unwrap();
        assert_eq!(
            pin_backoff_seconds(f),
            t["waitSeconds"].as_u64().unwrap(),
            "failures {f}"
        );
    }
    assert_eq!(pin_backoff_seconds(u64::MAX), 3600);
}
