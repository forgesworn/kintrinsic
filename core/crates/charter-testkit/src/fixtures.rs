//! Well-formed sample wire values + deterministic test signers shared across
//! crate test suites.

use charter_primitives::{Nonce, PubKey, ReqId, Sha256Hex};
use charter_sys::signer::{MachineSigner, SeedSigner};

/// The pinned guardian test signer (seed `0x11`).
pub fn guardian() -> SeedSigner {
    SeedSigner::from_seed(0x11)
}

/// The machine test signer (seed `0x22`).
pub fn machine() -> SeedSigner {
    SeedSigner::from_seed(0x22)
}

/// The guardian's pinned x-only pubkey.
pub fn guardian_pubkey() -> PubKey {
    MachineSigner::pubkey(&guardian())
}

/// The machine's x-only pubkey.
pub fn machine_pubkey() -> PubKey {
    MachineSigner::pubkey(&machine())
}

/// A deterministic request id from a single seed byte.
pub fn req_id(seed: u8) -> ReqId {
    ReqId::from_bytes([seed; 32])
}

/// A deterministic nonce from a single seed byte.
pub fn nonce(seed: u8) -> Nonce {
    Nonce::from_bytes([seed; 32])
}

/// The sha256 of `data` as a [`Sha256Hex`].
pub fn sha256_of(data: &[u8]) -> Sha256Hex {
    Sha256Hex::from_bytes(charter_crypto::sha256(data))
}
