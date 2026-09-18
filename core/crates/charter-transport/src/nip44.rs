//! NIP-44 v2 encryption over the single `charter-crypto` ECDH backend +
//! RustCrypto ChaCha20 / HKDF / HMAC. Payload layout:
//! `base64(0x02 || nonce[32] || ciphertext || mac[32])`, MAC over `nonce ||
//! ciphertext`, message keys = `HKDF-Expand(conversation_key, nonce, 76)`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
use hkdf::Hkdf;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const VERSION: u8 = 2;
const SALT: &[u8] = b"nip44-v2";
const MIN_PLAINTEXT: usize = 1;
const MAX_PLAINTEXT: usize = 65535;

/// NIP-44 errors. Decrypt failures never leak plaintext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nip44Error {
    /// Base64 decode failed.
    NotBase64,
    /// Unsupported version byte.
    BadVersion,
    /// Payload too short or too long.
    BadLength,
    /// MAC did not verify.
    BadMac,
    /// Padding / length prefix invalid.
    BadPadding,
    /// Plaintext was not valid UTF-8.
    BadUtf8,
    /// ECDH / key derivation failed.
    BadKey,
}

/// The shared conversation key: `HKDF-Extract(salt="nip44-v2", ikm=ecdh_x)`.
pub fn conversation_key(secret32: &[u8; 32], pubkey32: &[u8; 32]) -> Result<[u8; 32], Nip44Error> {
    let shared_x = charter_crypto::ecdh_x(secret32, pubkey32).ok_or(Nip44Error::BadKey)?;
    let (prk, _) = Hkdf::<Sha256>::extract(Some(SALT), &shared_x);
    let mut out = [0u8; 32];
    out.copy_from_slice(&prk);
    Ok(out)
}

fn message_keys(conversation_key: &[u8; 32], nonce: &[u8; 32]) -> ([u8; 32], [u8; 12], [u8; 32]) {
    let hk = Hkdf::<Sha256>::from_prk(conversation_key).expect("32-byte prk");
    let mut okm = [0u8; 76];
    hk.expand(nonce, &mut okm).expect("76-byte expand");
    let mut chacha_key = [0u8; 32];
    let mut chacha_nonce = [0u8; 12];
    let mut hmac_key = [0u8; 32];
    chacha_key.copy_from_slice(&okm[0..32]);
    chacha_nonce.copy_from_slice(&okm[32..44]);
    hmac_key.copy_from_slice(&okm[44..76]);
    (chacha_key, chacha_nonce, hmac_key)
}

fn calc_padded_len(unpadded: usize) -> usize {
    if unpadded <= 32 {
        return 32;
    }
    let log = (unpadded - 1) as u64;
    let next_power = 1usize << (64 - log.leading_zeros());
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    chunk * ((unpadded - 1) / chunk + 1)
}

fn pad(data: &[u8]) -> Vec<u8> {
    let unpadded = data.len();
    let padded_len = calc_padded_len(unpadded);
    let mut out = Vec::with_capacity(2 + padded_len);
    out.extend_from_slice(&(unpadded as u16).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(2 + padded_len, 0);
    out
}

fn mac(hmac_key: &[u8; 32], nonce: &[u8; 32], ciphertext: &[u8]) -> [u8; 32] {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(hmac_key).expect("hmac key");
    m.update(nonce);
    m.update(ciphertext);
    let out = m.finalize().into_bytes();
    let mut mac = [0u8; 32];
    mac.copy_from_slice(&out);
    mac
}

/// Encrypt `plaintext` with `conversation_key` and a 32-byte `nonce`.
pub fn encrypt(
    plaintext: &str,
    conversation_key: &[u8; 32],
    nonce: &[u8; 32],
) -> Result<String, Nip44Error> {
    let bytes = plaintext.as_bytes();
    if bytes.len() < MIN_PLAINTEXT || bytes.len() > MAX_PLAINTEXT {
        return Err(Nip44Error::BadLength);
    }
    let (chacha_key, chacha_nonce, hmac_key) = message_keys(conversation_key, nonce);
    let mut buf = pad(bytes);
    let mut cipher = ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into());
    cipher.apply_keystream(&mut buf);
    let tag = mac(&hmac_key, nonce, &buf);

    let mut payload = Vec::with_capacity(1 + 32 + buf.len() + 32);
    payload.push(VERSION);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(&buf);
    payload.extend_from_slice(&tag);
    Ok(STANDARD.encode(payload))
}

/// Decrypt a NIP-44 v2 payload. Any error yields no plaintext.
pub fn decrypt(payload_b64: &str, conversation_key: &[u8; 32]) -> Result<String, Nip44Error> {
    // Reject the future "encoded as #" marker form outright.
    if payload_b64.starts_with('#') {
        return Err(Nip44Error::BadVersion);
    }
    let raw = STANDARD
        .decode(payload_b64)
        .map_err(|_| Nip44Error::NotBase64)?;
    // 1 (version) + 32 (nonce) + 34 (min ciphertext = 2 + 32) + 32 (mac) = 99.
    if raw.len() < 99 || raw.len() > 65603 {
        return Err(Nip44Error::BadLength);
    }
    if raw[0] != VERSION {
        return Err(Nip44Error::BadVersion);
    }
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&raw[1..33]);
    let ct_end = raw.len() - 32;
    let ciphertext = &raw[33..ct_end];
    let claimed_mac = &raw[ct_end..];

    let (chacha_key, chacha_nonce, hmac_key) = message_keys(conversation_key, &nonce);
    let expected = mac(&hmac_key, &nonce, ciphertext);
    // Constant-time comparison.
    if !constant_time_eq(&expected, claimed_mac) {
        return Err(Nip44Error::BadMac);
    }

    let mut buf = ciphertext.to_vec();
    let mut cipher = ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into());
    cipher.apply_keystream(&mut buf);

    if buf.len() < 2 {
        return Err(Nip44Error::BadPadding);
    }
    let unpadded_len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if unpadded_len < MIN_PLAINTEXT
        || unpadded_len > buf.len() - 2
        || buf.len() != 2 + calc_padded_len(unpadded_len)
    {
        return Err(Nip44Error::BadPadding);
    }
    let plaintext = &buf[2..2 + unpadded_len];
    String::from_utf8(plaintext.to_vec()).map_err(|_| Nip44Error::BadUtf8)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_len_boundaries() {
        assert_eq!(calc_padded_len(1), 32);
        assert_eq!(calc_padded_len(32), 32);
        assert_eq!(calc_padded_len(33), 64);
        assert_eq!(calc_padded_len(100), 128);
    }

    #[test]
    fn roundtrip() {
        let ck = [7u8; 32];
        let nonce = [9u8; 32];
        let payload = encrypt("hello", &ck, &nonce).unwrap();
        assert_eq!(decrypt(&payload, &ck).unwrap(), "hello");
    }

    // CROSS-LANGUAGE PARITY for the offline guardian unlock. charterd computes
    // the code from the MACHINE side (machine_sk + guardian_pk); MyCharter (TS,
    // noble) computes it from the GUARDIAN side (guardian_sk + machine_pk). ECDH
    // symmetry + identical NIP-44 v2 across secp256k1/noble must make them equal
    // — otherwise a parent's code silently never unlocks a real device. The TS
    // mirror `apps/charter-app/src/unlock.test.ts` asserts the SAME "83000851".
    #[test]
    fn offline_unlock_code_matches_ts_guardian_side() {
        let machine_sk = [9u8; 32];
        let guardian_sk = [3u8; 32];
        let guardian_pk = charter_crypto::xonly_pubkey(&guardian_sk).unwrap();
        let conv = conversation_key(&machine_sk, &guardian_pk).unwrap();
        assert_eq!(
            charter_crypto::unlock::unlock_code(&conv, "Z9F2"),
            "83000851"
        );
    }
}
