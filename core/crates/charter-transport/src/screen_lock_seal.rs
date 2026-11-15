//! The sealed PIN of a SCREEN_LOCK_RESET (`spec/contract.md`, "Screen-lock
//! reset"). The guardian seals the new PIN to the machine key with a one-use
//! key of its own, before anything is signed, so a remote NIP-46 signer that
//! signs and encrypts the reset for the guardian only ever sees ciphertext.
//! The device opens it here, with its machine key and the payload's `epk`.
//!
//! The seal is NIP-44 v2 under `conversation_key(ephemeral, machine)`; its
//! plaintext is the PIN as a JSON string (`charter_proto::
//! screen_lock_pin_plaintext`). Errors are unit variants and nothing here
//! prints a PIN.

use charter_primitives::PubKey;
use charter_proto::{
    screen_lock_pin_from_plaintext, screen_lock_pin_plaintext, ScreenLockResetPayload,
};
use zeroize::Zeroizing;

use crate::nip44;

/// Why a sealed PIN could not be made or opened. Never carries the PIN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockSealError {
    /// A key is not usable: `epk` is not a curve point, or a secret is not a
    /// valid scalar.
    BadKey,
    /// The seal would not open with this machine's key: made for another
    /// machine, or altered.
    Undecryptable,
    /// It opened, but held something other than `""` or 4 to 16 ASCII digits
    /// in the agreed form.
    BadPin,
}

impl ScreenLockSealError {
    /// The unit name, for diagnostics that must never carry the payload.
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenLockSealError::BadKey => "bad_key",
            ScreenLockSealError::Undecryptable => "undecryptable",
            ScreenLockSealError::BadPin => "bad_pin",
        }
    }
}

/// Seal `pin` (`""` or 4 to 16 ASCII digits) to `machine` with the one-use
/// `ephemeral_secret` and `nonce`. Returns `(epk, newPinSealed)`.
///
/// The guardian app does this in TypeScript; this side exists for the
/// golden vector and for tests.
pub fn seal_screen_lock_pin(
    pin: &str,
    ephemeral_secret: &[u8; 32],
    machine: &PubKey,
    nonce: &[u8; 32],
) -> Result<(PubKey, String), ScreenLockSealError> {
    let plaintext =
        Zeroizing::new(screen_lock_pin_plaintext(pin).ok_or(ScreenLockSealError::BadPin)?);
    let epk =
        charter_crypto::xonly_pubkey(ephemeral_secret).map_err(|_| ScreenLockSealError::BadKey)?;
    let key = nip44::conversation_key_zeroizing(ephemeral_secret, machine.as_bytes())
        .map_err(|_| ScreenLockSealError::BadKey)?;
    let sealed =
        nip44::encrypt(&plaintext, &key, nonce).map_err(|_| ScreenLockSealError::BadPin)?;
    Ok((PubKey::from_bytes(epk), sealed))
}

/// Open the sealed PIN of an authenticated reset with this machine's secret
/// key. `Ok` holds `""` (remove the screen lock) or 4 to 16 ASCII digits, and
/// is wiped when dropped.
pub fn open_screen_lock_pin(
    machine_secret: &[u8; 32],
    payload: &ScreenLockResetPayload,
) -> Result<Zeroizing<String>, ScreenLockSealError> {
    let key = nip44::conversation_key_zeroizing(machine_secret, payload.epk.as_bytes())
        .map_err(|_| ScreenLockSealError::BadKey)?;
    let plaintext = nip44::decrypt_zeroizing(&payload.new_pin_sealed, &key)
        .map_err(|_| ScreenLockSealError::Undecryptable)?;
    let pin = screen_lock_pin_from_plaintext(&plaintext).ok_or(ScreenLockSealError::BadPin)?;
    Ok(Zeroizing::new(pin.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE_SK: [u8; 32] = [0x11; 32];
    const EPHEMERAL_SK: [u8; 32] = [0x22; 32];
    const OTHER_SK: [u8; 32] = [0x33; 32];
    const NONCE: [u8; 32] = [0xA1; 32];

    fn pk(sk: &[u8; 32]) -> PubKey {
        PubKey::from_bytes(charter_crypto::xonly_pubkey(sk).unwrap())
    }

    fn payload(epk: PubKey, sealed: String) -> ScreenLockResetPayload {
        ScreenLockResetPayload {
            v: 1,
            id: "0123456789abcdef0123456789abcdef".into(),
            machine: pk(&MACHINE_SK),
            issued_at: 1_782_734_400,
            epk,
            new_pin_sealed: sealed,
        }
    }

    #[test]
    fn a_seal_opens_with_the_machine_key_only() {
        for pin in ["", "0000", "123456", "1234567890123456"] {
            let (epk, sealed) =
                seal_screen_lock_pin(pin, &EPHEMERAL_SK, &pk(&MACHINE_SK), &NONCE).unwrap();
            assert_eq!(epk, pk(&EPHEMERAL_SK));
            // Every PIN, and a removal, seals to the same length.
            assert_eq!(sealed.len(), 132, "{pin:?}");
            let p = payload(epk, sealed);
            assert_eq!(open_screen_lock_pin(&MACHINE_SK, &p).unwrap().as_str(), pin);
            assert_eq!(
                open_screen_lock_pin(&OTHER_SK, &p).map(|_| ()),
                Err(ScreenLockSealError::Undecryptable)
            );
        }
    }

    #[test]
    fn a_pin_the_device_would_refuse_is_never_sealed() {
        for bad in ["123", "12345678901234567", "12a4", "1234 "] {
            assert_eq!(
                seal_screen_lock_pin(bad, &EPHEMERAL_SK, &pk(&MACHINE_SK), &NONCE),
                Err(ScreenLockSealError::BadPin)
            );
        }
    }

    #[test]
    fn an_opened_seal_that_holds_no_pin_is_refused() {
        let key = nip44::conversation_key(&EPHEMERAL_SK, pk(&MACHINE_SK).as_bytes()).unwrap();
        for plain in ["\"123\"", "123456", "\"12a4\"", "{\"newPin\":\"1234\"}"] {
            let sealed = nip44::encrypt(plain, &key, &NONCE).unwrap();
            assert_eq!(
                open_screen_lock_pin(&MACHINE_SK, &payload(pk(&EPHEMERAL_SK), sealed)).map(|_| ()),
                Err(ScreenLockSealError::BadPin),
                "{plain:?}"
            );
        }
    }

    #[test]
    fn an_epk_off_the_curve_is_a_bad_key() {
        let (_, sealed) =
            seal_screen_lock_pin("1234", &EPHEMERAL_SK, &pk(&MACHINE_SK), &NONCE).unwrap();
        let p = payload(PubKey::from_bytes([0xFF; 32]), sealed);
        assert_eq!(
            open_screen_lock_pin(&MACHINE_SK, &p).map(|_| ()),
            Err(ScreenLockSealError::BadKey)
        );
    }
}
