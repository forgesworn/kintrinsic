//! The Android guardian PIN: the record a guardian sets inside RELAYS'
//! `guardianPin`, its scrypt verification, and the wrong-entry backoff.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{constant_time_eq, decode_hex_vec, encode_hex, is_hex64, is_one};

/// scrypt cost: N = 2^15.
pub const PIN_SCRYPT_LOG_N: u8 = 15;
pub const PIN_SCRYPT_R: u32 = 8;
pub const PIN_SCRYPT_P: u32 = 1;
pub const PIN_SCRYPT_DK_LEN: usize = 32;
/// Failures allowed before backoff starts.
pub const PIN_FREE_FAILURES: u64 = 5;
/// First wait, in seconds, after the free failures.
pub const PIN_BACKOFF_BASE_SECONDS: u64 = 60;
/// Backoff cap, in seconds.
pub const PIN_BACKOFF_CAP_SECONDS: u64 = 3600;

/// A guardian PIN record `{ v: 1, salt, hash }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardianPinRecord {
    pub v: u8,
    /// Lowercase hex, 16 to 64 bytes.
    pub salt: String,
    /// Lowercase hex, 32 bytes.
    pub hash: String,
}

/// Validate a `guardianPin` value: an object with `v` = 1, `salt` lowercase
/// hex of 16 to 64 bytes and `hash` 64 lowercase hex. Extra members are
/// ignored and not kept.
pub fn validate_pin_record(x: &Value) -> Option<GuardianPinRecord> {
    let o = x.as_object()?;
    if !o.get("v").is_some_and(is_one) {
        return None;
    }
    let salt = o.get("salt")?.as_str()?;
    let salt_ok = salt.len().is_multiple_of(2)
        && (32..=128).contains(&salt.len())
        && salt.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    if !salt_ok {
        return None;
    }
    let hash = o.get("hash")?.as_str()?;
    if !is_hex64(hash) {
        return None;
    }
    Some(GuardianPinRecord {
        v: 1,
        salt: salt.to_owned(),
        hash: hash.to_owned(),
    })
}

/// scrypt(UTF-8 `pin`, `salt`, N = 2^15, r = 8, p = 1, 32 bytes).
pub fn pin_hash(pin: &str, salt: &[u8]) -> [u8; 32] {
    // The output length is the buffer's: `PIN_SCRYPT_DK_LEN`.
    let params = scrypt::Params::new(PIN_SCRYPT_LOG_N, PIN_SCRYPT_R, PIN_SCRYPT_P)
        .expect("fixed scrypt parameters are valid");
    let mut out = [0u8; PIN_SCRYPT_DK_LEN];
    scrypt::scrypt(pin.as_bytes(), salt, &params, &mut out).expect("32-byte output is valid");
    out
}

/// Whether `pin` matches `record` (compared in constant time). A record whose
/// salt does not decode never matches.
pub fn verify_pin(record: &GuardianPinRecord, pin: &str) -> bool {
    let Some(salt) = decode_hex_vec(&record.salt) else {
        return false;
    };
    constant_time_eq(&encode_hex(&pin_hash(pin, &salt)), &record.hash)
}

/// Seconds the ward must wait before the next PIN try after `failures`
/// consecutive wrong entries: 0 below 5, then 60 × 2^(failures − 5), capped
/// at 3600.
pub fn pin_backoff_seconds(failures: u64) -> u64 {
    if failures < PIN_FREE_FAILURES {
        return 0;
    }
    let exp = failures - PIN_FREE_FAILURES;
    if exp >= 6 {
        return PIN_BACKOFF_CAP_SECONDS;
    }
    (PIN_BACKOFF_BASE_SECONDS << exp).min(PIN_BACKOFF_CAP_SECONDS)
}

/// The earliest Unix second the next try is allowed, from the last failure.
pub fn pin_next_try_at(failures: u64, last_failure_at: i64) -> i64 {
    last_failure_at.saturating_add(pin_backoff_seconds(failures) as i64)
}
