//! The SCREEN_LOCK_RESET payload (kind `CHARTER_DEVICE_SCREEN_LOCK_RESET`,
//! 31119): the guardian's way into a ward phone. The guardian signs it and
//! gift-wraps it to one machine, exactly like a RELEASE; the device
//! authenticates the inner signature, confirms it targets THIS machine, and
//! hands the new screen lock to the platform, which tells the ward.
//!
//! The payload carries a PIN, so nothing here ever prints one: [`Debug`] is
//! redacted, and a refused payload says only WHICH rule it broke
//! ([`ScreenLockPayloadError`]), never serde's message, which can quote the
//! offending value.

use std::fmt;

use serde::{Deserialize, Serialize};

use charter_primitives::PubKey;

/// The only payload version this build implements.
pub const SCREEN_LOCK_RESET_VERSION: u32 = 1;
/// Length of a reset `id`: 16 random bytes as lowercase hex.
pub const SCREEN_LOCK_RESET_ID_LEN: usize = 32;
/// Fewest digits a new PIN may have.
pub const SCREEN_LOCK_PIN_MIN_DIGITS: usize = 4;
/// Most digits a new PIN may have.
pub const SCREEN_LOCK_PIN_MAX_DIGITS: usize = 16;

/// The decoded SCREEN_LOCK_RESET content (JSON, `camelCase`). Field order is
/// the wire order: `{"v","id","machine","issuedAt","newPin"}`. Unknown fields
/// are tolerated, as on a RELEASE.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenLockResetPayload {
    pub v: u32,
    /// The guardian's random id for this reset, 32 lowercase hex characters.
    /// The device reports the outcome under it (STATUS `screenLockResets`).
    pub id: String,
    /// The device this reset targets: it must equal the receiver's machine
    /// pubkey, so one device's reset can never change another's lock.
    pub machine: PubKey,
    /// The guardian's true wall clock when it signed (unix seconds). The
    /// device bounds freshness on this field alone.
    pub issued_at: u64,
    /// The new PIN, 4 to 16 ASCII digits, or `""` to remove the screen lock.
    pub new_pin: String,
}

/// Why a payload was refused. Deliberately carries no text from the payload:
/// a PIN must never reach a log line or a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockPayloadError {
    /// Not JSON, a missing field, or a field of the wrong type.
    Json,
    /// `v` is not [`SCREEN_LOCK_RESET_VERSION`].
    BadVersion,
    /// `id` is not 32 lowercase hex characters.
    BadId,
    /// `newPin` is neither `""` nor 4 to 16 ASCII digits.
    BadNewPin,
}

impl fmt::Debug for ScreenLockResetPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenLockResetPayload")
            .field("v", &self.v)
            .field("id", &self.id)
            .field("machine", &self.machine)
            .field("issued_at", &self.issued_at)
            .field("removes_screen_lock", &self.removes_screen_lock())
            .finish_non_exhaustive()
    }
}

impl ScreenLockResetPayload {
    /// Serialize to compact JSON, in wire field order.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("screen-lock reset serializes")
    }

    /// Parse and validate. Every `Err` means "do not reset".
    pub fn from_json(s: &str) -> Result<ScreenLockResetPayload, ScreenLockPayloadError> {
        let p: ScreenLockResetPayload =
            serde_json::from_str(s).map_err(|_| ScreenLockPayloadError::Json)?;
        if p.v != SCREEN_LOCK_RESET_VERSION {
            return Err(ScreenLockPayloadError::BadVersion);
        }
        if !is_valid_reset_id(&p.id) {
            return Err(ScreenLockPayloadError::BadId);
        }
        if !is_valid_new_pin(&p.new_pin) {
            return Err(ScreenLockPayloadError::BadNewPin);
        }
        Ok(p)
    }

    /// `true` when this reset removes the screen lock (`newPin` is `""`).
    pub fn removes_screen_lock(&self) -> bool {
        self.new_pin.is_empty()
    }
}

/// A reset id: exactly 32 lowercase hex characters.
pub fn is_valid_reset_id(id: &str) -> bool {
    id.len() == SCREEN_LOCK_RESET_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A new PIN: `""` (remove the screen lock) or 4 to 16 ASCII digits.
pub fn is_valid_new_pin(pin: &str) -> bool {
    pin.is_empty()
        || ((SCREEN_LOCK_PIN_MIN_DIGITS..=SCREEN_LOCK_PIN_MAX_DIGITS).contains(&pin.len())
            && pin.bytes().all(|b| b.is_ascii_digit()))
}

/// The outcome the device records for one reset id: what STATUS
/// `screenLockResets[].result` and the `screenlock.reset` AUDIT `outcome`
/// carry. Exactly one is recorded per id; `accepted` is the only one later
/// replaced, by the platform's final answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenLockResetResult {
    /// Authenticated and handed to the platform; no final answer yet.
    Accepted,
    /// The platform changed (or removed) the screen lock.
    Applied,
    /// The platform refused or threw.
    Failed,
    /// The device's reset token is not active yet (it activates at the next
    /// unlock after it is set).
    NotReady,
    /// The device could not write the ward's notice, so it did not reset.
    NoticeFailed,
    /// `issuedAt` was outside the device's acceptance window: the clocks
    /// disagree, or the reset arrived more than an hour late.
    RejectedTime,
    /// A newer reset was accepted first, or replaced this one before the
    /// platform took it.
    Superseded,
}

impl ScreenLockResetResult {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenLockResetResult::Accepted => "accepted",
            ScreenLockResetResult::Applied => "applied",
            ScreenLockResetResult::Failed => "failed",
            ScreenLockResetResult::NotReady => "not_ready",
            ScreenLockResetResult::NoticeFailed => "notice_failed",
            ScreenLockResetResult::RejectedTime => "rejected_time",
            ScreenLockResetResult::Superseded => "superseded",
        }
    }

    /// Parse a wire spelling; `None` for anything else.
    pub fn parse(s: &str) -> Option<ScreenLockResetResult> {
        Some(match s {
            "accepted" => ScreenLockResetResult::Accepted,
            "applied" => ScreenLockResetResult::Applied,
            "failed" => ScreenLockResetResult::Failed,
            "not_ready" => ScreenLockResetResult::NotReady,
            "notice_failed" => ScreenLockResetResult::NoticeFailed,
            "rejected_time" => ScreenLockResetResult::RejectedTime,
            "superseded" => ScreenLockResetResult::Superseded,
            _ => return None,
        })
    }

    /// One of the four results the platform reports after taking a reset.
    pub fn is_platform_report(self) -> bool {
        matches!(
            self,
            ScreenLockResetResult::Applied
                | ScreenLockResetResult::Failed
                | ScreenLockResetResult::NotReady
                | ScreenLockResetResult::NoticeFailed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn sample(new_pin: &str) -> ScreenLockResetPayload {
        ScreenLockResetPayload {
            v: 1,
            id: ID.into(),
            machine: PubKey::from_bytes([0xAB; 32]),
            issued_at: 1_700_000_000,
            new_pin: new_pin.into(),
        }
    }

    #[test]
    fn roundtrips_in_wire_order() {
        let p = sample("123456");
        let json = p.to_json();
        assert_eq!(
            json,
            format!(
                "{{\"v\":1,\"id\":\"{ID}\",\"machine\":\"{}\",\"issuedAt\":1700000000,\"newPin\":\"123456\"}}",
                "ab".repeat(32)
            )
        );
        assert_eq!(ScreenLockResetPayload::from_json(&json).unwrap(), p);
    }

    #[test]
    fn an_empty_pin_removes_the_screen_lock() {
        let p = ScreenLockResetPayload::from_json(&sample("").to_json()).unwrap();
        assert!(p.removes_screen_lock());
        assert!(!sample("1234").removes_screen_lock());
    }

    #[test]
    fn pin_length_and_digits_are_bounded() {
        for ok in ["", "1234", "0000", "1234567890123456"] {
            assert!(is_valid_new_pin(ok), "{ok:?}");
        }
        for bad in [
            "123",
            "12345678901234567",
            "12a4",
            " 1234",
            "1234 ",
            "١٢٣٤",
            "+1234",
        ] {
            assert!(!is_valid_new_pin(bad), "{bad:?}");
            assert_eq!(
                ScreenLockResetPayload::from_json(&sample(bad).to_json()),
                Err(ScreenLockPayloadError::BadNewPin)
            );
        }
    }

    #[test]
    fn id_is_exactly_32_lowercase_hex() {
        assert!(is_valid_reset_id(ID));
        for bad in [
            "",
            &ID[..31],
            &format!("{ID}0"),
            &ID.to_uppercase(),
            "g123456789abcdef0123456789abcdef",
        ] {
            assert!(!is_valid_reset_id(bad), "{bad:?}");
            let mut p = sample("1234");
            p.id = bad.to_string();
            assert_eq!(
                ScreenLockResetPayload::from_json(&p.to_json()),
                Err(ScreenLockPayloadError::BadId)
            );
        }
    }

    #[test]
    fn version_must_be_one() {
        let mut p = sample("1234");
        p.v = 2;
        assert_eq!(
            ScreenLockResetPayload::from_json(&p.to_json()),
            Err(ScreenLockPayloadError::BadVersion)
        );
    }

    #[test]
    fn unknown_fields_are_tolerated_like_a_release() {
        let mut v: serde_json::Value = serde_json::from_str(&sample("1234").to_json()).unwrap();
        v["later"] = serde_json::json!(true);
        assert!(ScreenLockResetPayload::from_json(&v.to_string()).is_ok());
    }

    /// A PIN must never reach a diagnostic: not through `Debug`, and not
    /// through a parse error, where serde would quote a mistyped value.
    #[test]
    fn no_pin_in_debug_or_errors() {
        let p = sample("987654");
        assert!(!format!("{p:?}").contains("987654"), "{p:?}");
        let mistyped = format!(
            "{{\"v\":1,\"id\":\"{ID}\",\"machine\":\"{}\",\"issuedAt\":1,\"newPin\":987654}}",
            "ab".repeat(32)
        );
        let e = ScreenLockResetPayload::from_json(&mistyped).unwrap_err();
        assert_eq!(e, ScreenLockPayloadError::Json);
        assert!(!format!("{e:?}").contains("987654"));
    }

    #[test]
    fn result_spellings_roundtrip() {
        use ScreenLockResetResult as R;
        for r in [
            R::Accepted,
            R::Applied,
            R::Failed,
            R::NotReady,
            R::NoticeFailed,
            R::RejectedTime,
            R::Superseded,
        ] {
            assert_eq!(R::parse(r.as_str()), Some(r));
            assert_eq!(
                serde_json::to_string(&r).unwrap(),
                format!("\"{}\"", r.as_str())
            );
        }
        assert_eq!(R::parse("APPLIED"), None);
        let reports: Vec<_> = [
            R::Accepted,
            R::Applied,
            R::Failed,
            R::NotReady,
            R::NoticeFailed,
            R::RejectedTime,
            R::Superseded,
        ]
        .into_iter()
        .filter(|r| r.is_platform_report())
        .collect();
        assert_eq!(
            reports,
            vec![R::Applied, R::Failed, R::NotReady, R::NoticeFailed]
        );
    }
}
