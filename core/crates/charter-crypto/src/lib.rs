//! `charter-crypto` — the **one** schnorr + sha256 backend for the whole
//! workspace. Both `charter-verify` (grant/clause authentication) and
//! `charter-transport` (NIP-44 / NIP-59) consume this single BIP-340 wrapper.
//! There is no second crypto backend anywhere in the tree.
//!
//! All functions take and return fixed-width byte arrays so callers never
//! touch `secp256k1` types directly.

use secp256k1::schnorr::Signature;
use secp256k1::{Keypair, Parity, PublicKey, SecretKey, XOnlyPublicKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub mod unlock;

/// Errors from the crypto wrapper. Verification never errors (it returns
/// `false`); only key/signing construction can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// A 32-byte secret key was not a valid scalar.
    BadSecretKey,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoError::BadSecretKey => write!(f, "invalid secret key"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// BIP-340 schnorr verification. Returns `false` for any malformed input
/// (bad pubkey, bad signature) or a failed check — never panics, never errors.
pub fn schnorr_verify(pubkey32: &[u8; 32], msg32: &[u8; 32], sig64: &[u8; 64]) -> bool {
    let pk = match XOnlyPublicKey::from_byte_array(*pubkey32) {
        Ok(pk) => pk,
        Err(_) => return false,
    };
    // 0.33's from_byte_array is infallible: any 64 bytes are a syntactically
    // valid signature, and a bad one simply fails verification below, so this
    // still returns false for malformed input exactly as before.
    let sig = Signature::from_byte_array(*sig64);
    secp256k1::schnorr::verify(&sig, msg32, &pk).is_ok()
}

/// Derive the 32-byte x-only public key for a secret key.
pub fn xonly_pubkey(secret32: &[u8; 32]) -> Result<[u8; 32], CryptoError> {
    let sk = SecretKey::from_secret_bytes(*secret32).map_err(|_| CryptoError::BadSecretKey)?;
    let kp = Keypair::from_secret_key(&sk);
    Ok(kp.x_only_public_key().0.to_byte_array())
}

/// The x-coordinate of the ECDH shared point `secret * pubkey`, where `pubkey`
/// is an x-only key lifted with even y (NIP-44 v2 convention). Returns `None`
/// for an invalid secret or pubkey.
///
/// Returned wrapped in [`Zeroizing`]: this is shared secret material (the
/// NIP-44 conversation key is derived directly from it) and must not linger
/// in freed memory after the caller drops it.
pub fn ecdh_x(secret32: &[u8; 32], xonly_pub32: &[u8; 32]) -> Option<Zeroizing<[u8; 32]>> {
    let sk = SecretKey::from_secret_bytes(*secret32).ok()?;
    let xonly = XOnlyPublicKey::from_byte_array(*xonly_pub32).ok()?;
    let full: PublicKey = xonly.public_key(Parity::Even);
    let point = secp256k1::ecdh::shared_secret_point(&full, &sk);
    let mut x = Zeroizing::new([0u8; 32]);
    x.copy_from_slice(&point[..32]);
    Some(x)
}

/// Deterministic (no-aux-rand) BIP-340 schnorr signature over `msg32`.
///
/// Deterministic signing is used so test vectors reproduce; production callers
/// that want aux randomness should still verify against a generated vector
/// rather than reproducing a signature byte-for-byte.
pub fn schnorr_sign(secret32: &[u8; 32], msg32: &[u8; 32]) -> Result<[u8; 64], CryptoError> {
    let sk = SecretKey::from_secret_bytes(*secret32).map_err(|_| CryptoError::BadSecretKey)?;
    let kp = Keypair::from_secret_key(&sk);
    let sig = secp256k1::schnorr::sign_no_aux_rand(msg32, &kp);
    Ok(*sig.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC-style fixed test secret (a valid, non-zero scalar below the order).
    fn secret() -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = 1;
        s
    }

    #[test]
    fn sha256_known_answer() {
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let got = sha256(b"");
        let want = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let sk = secret();
        let pk = xonly_pubkey(&sk).unwrap();
        let msg = sha256(b"charter device grant");
        let sig = schnorr_sign(&sk, &msg).unwrap();
        assert!(schnorr_verify(&pk, &msg, &sig));
    }

    #[test]
    fn schnorr_verify_rejects_flipped_sig_bit() {
        let sk = secret();
        let pk = xonly_pubkey(&sk).unwrap();
        let msg = sha256(b"charter device grant");
        let mut sig = schnorr_sign(&sk, &msg).unwrap();
        sig[0] ^= 0x01;
        assert!(!schnorr_verify(&pk, &msg, &sig));
    }

    #[test]
    fn schnorr_verify_rejects_wrong_pubkey() {
        let sk = secret();
        let msg = sha256(b"charter device grant");
        let sig = schnorr_sign(&sk, &msg).unwrap();
        let mut other = secret();
        other[31] = 2;
        let wrong_pk = xonly_pubkey(&other).unwrap();
        assert!(!schnorr_verify(&wrong_pk, &msg, &sig));
    }

    #[test]
    fn schnorr_verify_rejects_wrong_message() {
        let sk = secret();
        let pk = xonly_pubkey(&sk).unwrap();
        let sig = schnorr_sign(&sk, &sha256(b"a")).unwrap();
        assert!(!schnorr_verify(&pk, &sha256(b"b"), &sig));
    }

    #[test]
    fn bad_secret_key_rejected() {
        // All-zero scalar is invalid.
        assert_eq!(xonly_pubkey(&[0u8; 32]), Err(CryptoError::BadSecretKey));
    }
}
