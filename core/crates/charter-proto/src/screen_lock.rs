//! The SCREEN_LOCK_RESET payload (kind `CHARTER_DEVICE_SCREEN_LOCK_RESET`,
//! 31122): the guardian's way into a ward phone. The guardian signs it and
//! gift-wraps it to one machine, exactly like a RELEASE; the device
//! authenticates the inner signature, confirms it targets THIS machine, opens
//! the sealed PIN with its own key, and hands the new screen lock to the
//! platform, which tells the ward.
//!
//! The PIN is never in the signed content in the clear: it is SEALED to the
//! machine key (`newPinSealed`, NIP-44 v2 from a one-use guardian key whose
//! public half is `epk`), because a guardian who signs with a remote NIP-46
//! signer hands that signer the content to sign and to encrypt, and the signer
//! must never see the PIN. The sealed plaintext is the PIN as a JSON string
//! ([`screen_lock_pin_plaintext`]): `"123456"`, or `""` to remove the screen
//! lock, since a NIP-44 plaintext can never be empty.
//!
//! Nothing here ever prints a PIN or its ciphertext: [`Debug`] is redacted,
//! and a refused payload says only WHICH rule it broke
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
/// Longest `newPinSealed` accepted, in base64 characters. A sealed PIN pads
/// to NIP-44's 32-byte minimum, so every one is exactly 132 characters; the
/// bound only keeps a hostile payload small.
pub const SCREEN_LOCK_SEALED_MAX_LEN: usize = 512;

/// The decoded SCREEN_LOCK_RESET content (JSON, `camelCase`). Field order is
/// the wire order: `{"v","id","machine","issuedAt","epk","newPinSealed"}`.
/// Unknown fields are tolerated, as on a RELEASE.
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
    /// The x-only public half of the one-use key the guardian sealed the PIN
    /// with. Its secret half is discarded as soon as the seal is made.
    pub epk: PubKey,
    /// The new PIN sealed to `machine`: a NIP-44 v2 payload under the
    /// conversation key of `epk` and the machine key, whose plaintext is
    /// [`screen_lock_pin_plaintext`]. Only the device can open it.
    pub new_pin_sealed: String,
}

/// Why a payload was refused. Deliberately carries no text from the payload:
/// neither a PIN nor its ciphertext may reach a log line or a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockPayloadError {
    /// Not JSON, a missing field, or a field of the wrong type (`epk` and
    /// `machine` must be 64 lowercase hex characters).
    Json,
    /// `v` is not [`SCREEN_LOCK_RESET_VERSION`].
    BadVersion,
    /// `id` is not 32 lowercase hex characters.
    BadId,
    /// `newPinSealed` is empty, longer than [`SCREEN_LOCK_SEALED_MAX_LEN`],
    /// or not base64.
    BadSeal,
}

impl fmt::Debug for ScreenLockResetPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenLockResetPayload")
            .field("v", &self.v)
            .field("id", &self.id)
            .field("machine", &self.machine)
            .field("issued_at", &self.issued_at)
            .field("epk", &self.epk)
            .finish_non_exhaustive()
    }
}

impl ScreenLockResetPayload {
    /// Serialize to compact JSON, in wire field order.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("screen-lock reset serializes")
    }

    /// Parse and validate the shape. Every `Err` means "do not reset". The
    /// sealed PIN is checked only when the device opens it.
    pub fn from_json(s: &str) -> Result<ScreenLockResetPayload, ScreenLockPayloadError> {
        let p: ScreenLockResetPayload =
            serde_json::from_str(s).map_err(|_| ScreenLockPayloadError::Json)?;
        if p.v != SCREEN_LOCK_RESET_VERSION {
            return Err(ScreenLockPayloadError::BadVersion);
        }
        if !is_valid_reset_id(&p.id) {
            return Err(ScreenLockPayloadError::BadId);
        }
        if !is_plausible_seal(&p.new_pin_sealed) {
            return Err(ScreenLockPayloadError::BadSeal);
        }
        Ok(p)
    }
}

/// The shape a sealed PIN must have before the device tries to open it:
/// 1 to [`SCREEN_LOCK_SEALED_MAX_LEN`] standard base64 characters.
pub fn is_plausible_seal(sealed: &str) -> bool {
    !sealed.is_empty()
        && sealed.len() <= SCREEN_LOCK_SEALED_MAX_LEN
        && sealed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// The plaintext the guardian seals for `pin`: the PIN as a JSON string,
/// `"123456"`, or `""` to remove the screen lock (a NIP-44 plaintext cannot
/// be empty). `None` for a PIN the device would refuse. At most 18 bytes, so
/// every seal pads to the same length and says nothing about the PIN.
pub fn screen_lock_pin_plaintext(pin: &str) -> Option<String> {
    is_valid_new_pin(pin).then(|| format!("\"{pin}\""))
}

/// The PIN in an opened seal, which must be exactly the form
/// [`screen_lock_pin_plaintext`] makes: a double quote, `""` or 4 to 16 ASCII
/// digits, a double quote. Nothing else (no escapes, no spaces) is accepted.
pub fn screen_lock_pin_from_plaintext(plaintext: &str) -> Option<&str> {
    let pin = plaintext.strip_prefix('"')?.strip_suffix('"')?;
    is_valid_new_pin(pin).then_some(pin)
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
    /// The platform refused or threw, or the device could not open the
    /// sealed PIN (or what it held was not a PIN).
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
    /// A stand-in seal: the shape rules only (opening it is the device's job).
    const SEALED: &str = "AqGhoaGhoaGhoaGhoaGhoaGhoaGhoaGhoaGhoaGhoaGh";

    fn sample() -> ScreenLockResetPayload {
        ScreenLockResetPayload {
            v: 1,
            id: ID.into(),
            machine: PubKey::from_bytes([0xAB; 32]),
            issued_at: 1_700_000_000,
            epk: PubKey::from_bytes([0xCD; 32]),
            new_pin_sealed: SEALED.into(),
        }
    }

    #[test]
    fn roundtrips_in_wire_order() {
        let p = sample();
        let json = p.to_json();
        assert_eq!(
            json,
            format!(
                "{{\"v\":1,\"id\":\"{ID}\",\"machine\":\"{}\",\"issuedAt\":1700000000,\"epk\":\"{}\",\"newPinSealed\":\"{SEALED}\"}}",
                "ab".repeat(32),
                "cd".repeat(32)
            )
        );
        assert_eq!(ScreenLockResetPayload::from_json(&json).unwrap(), p);
    }

    /// The pre-seal shape (a plaintext `newPin`, no seal) is refused outright.
    #[test]
    fn a_plaintext_pin_without_a_seal_is_refused() {
        let plain = format!(
            "{{\"v\":1,\"id\":\"{ID}\",\"machine\":\"{}\",\"issuedAt\":1,\"newPin\":\"123456\"}}",
            "ab".repeat(32)
        );
        assert_eq!(
            ScreenLockResetPayload::from_json(&plain),
            Err(ScreenLockPayloadError::Json)
        );
    }

    #[test]
    fn the_seal_must_be_short_nonempty_base64() {
        for bad in [
            String::new(),
            "not base64!".to_string(),
            "A".repeat(SCREEN_LOCK_SEALED_MAX_LEN + 1),
        ] {
            let mut p = sample();
            p.new_pin_sealed = bad;
            assert_eq!(
                ScreenLockResetPayload::from_json(&p.to_json()),
                Err(ScreenLockPayloadError::BadSeal)
            );
        }
        let mut p = sample();
        p.new_pin_sealed = "A".repeat(SCREEN_LOCK_SEALED_MAX_LEN);
        assert!(ScreenLockResetPayload::from_json(&p.to_json()).is_ok());
    }

    #[test]
    fn epk_must_be_64_lowercase_hex() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json()).unwrap();
        for bad in ["cd".repeat(31), "CD".repeat(32), String::new()] {
            v["epk"] = serde_json::json!(bad);
            assert_eq!(
                ScreenLockResetPayload::from_json(&v.to_string()),
                Err(ScreenLockPayloadError::Json)
            );
        }
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
            assert_eq!(screen_lock_pin_plaintext(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_sealed_plaintext_is_the_pin_as_a_json_string() {
        for pin in ["", "1234", "1234567890123456"] {
            let plain = screen_lock_pin_plaintext(pin).unwrap();
            assert_eq!(plain, serde_json::to_string(pin).unwrap());
            assert!(plain.len() <= 18);
            assert_eq!(screen_lock_pin_from_plaintext(&plain), Some(pin));
        }
        for bad in [
            "",
            "\"",
            "123456",
            "\"123\"",
            "\"12345678901234567\"",
            "\"12a4\"",
            "\"1234 \"",
            " \"1234\"",
            "\"\\u0031234\"",
            "\"١٢٣٤\"",
            "{\"newPin\":\"1234\"}",
        ] {
            assert_eq!(screen_lock_pin_from_plaintext(bad), None, "{bad:?}");
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
            let mut p = sample();
            p.id = bad.to_string();
            assert_eq!(
                ScreenLockResetPayload::from_json(&p.to_json()),
                Err(ScreenLockPayloadError::BadId)
            );
        }
    }

    #[test]
    fn version_must_be_one() {
        let mut p = sample();
        p.v = 2;
        assert_eq!(
            ScreenLockResetPayload::from_json(&p.to_json()),
            Err(ScreenLockPayloadError::BadVersion)
        );
    }

    #[test]
    fn unknown_fields_are_tolerated_like_a_release() {
        let mut v: serde_json::Value = serde_json::from_str(&sample().to_json()).unwrap();
        v["later"] = serde_json::json!(true);
        assert!(ScreenLockResetPayload::from_json(&v.to_string()).is_ok());
    }

    /// Neither the seal nor a mistyped value may reach a diagnostic: not
    /// through `Debug`, and not through a parse error, where serde would quote
    /// the value.
    #[test]
    fn no_seal_in_debug_or_errors() {
        let p = sample();
        for shown in [format!("{p:?}"), format!("{p:#?}")] {
            assert!(!shown.contains(SEALED), "seal leaked: {shown}");
            assert!(!shown.contains("new_pin") && !shown.contains("newPin"));
        }
        let mistyped = format!(
            "{{\"v\":1,\"id\":\"{ID}\",\"machine\":\"{}\",\"issuedAt\":1,\"epk\":\"{}\",\"newPinSealed\":987654}}",
            "ab".repeat(32),
            "cd".repeat(32)
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
