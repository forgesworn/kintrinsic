//! Fixed-width hex newtypes for Charter wire primitives.
//!
//! Every type is a fixed-length byte array that serializes as a lowercase
//! hex string and deserializes with **strict** validation: exact length,
//! lowercase `[0-9a-f]` only. Uppercase, wrong length, or non-hex input is
//! rejected. There are no crypto or IO dependencies here.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Error decoding a fixed-width hex string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexError {
    /// The string was not exactly `2 * N` characters long.
    Length,
    /// The string contained a non-lowercase-hex character.
    Char,
}

impl fmt::Display for HexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HexError::Length => write!(f, "incorrect hex length"),
            HexError::Char => write!(f, "non-lowercase-hex character"),
        }
    }
}

impl std::error::Error for HexError {}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn hex_val(c: u8) -> Result<u8, HexError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(HexError::Char),
    }
}

pub(crate) fn from_hex_exact<const N: usize>(s: &str) -> Result<[u8; N], HexError> {
    if s.len() != N * 2 {
        return Err(HexError::Length);
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = hex_val(bytes[2 * i])?;
        let lo = hex_val(bytes[2 * i + 1])?;
        *slot = (hi << 4) | lo;
    }
    Ok(out)
}

macro_rules! hex_newtype {
    ($(#[$meta:meta])* $name:ident, $n:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; $n]);

        impl $name {
            /// Number of raw bytes (half the hex length).
            pub const LEN: usize = $n;

            /// Construct from raw bytes.
            pub const fn from_bytes(b: [u8; $n]) -> Self {
                Self(b)
            }

            /// Borrow the raw bytes.
            pub fn as_bytes(&self) -> &[u8; $n] {
                &self.0
            }

            /// Lowercase hex encoding.
            pub fn to_hex(&self) -> String {
                to_hex(&self.0)
            }

            /// Strict hex decode: exact length, lowercase hex only.
            pub fn from_hex(s: &str) -> Result<Self, HexError> {
                Ok(Self(from_hex_exact::<$n>(s)?))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.to_hex())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.to_hex())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_hex())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::from_hex(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_newtype!(
    /// A 256-bit brokered-request id.
    ReqId,
    32
);
hex_newtype!(
    /// A 256-bit request nonce.
    Nonce,
    32
);
hex_newtype!(
    /// A sha256 digest as 32 raw bytes (hex on the wire).
    Sha256Hex,
    32
);
hex_newtype!(
    /// A 32-byte x-only Nostr/BIP-340 public key.
    PubKey,
    32
);
hex_newtype!(
    /// A 32-byte NIP-01 event id.
    EventId,
    32
);
hex_newtype!(
    /// A 64-byte BIP-340 schnorr signature.
    Sig,
    64
);

#[cfg(test)]
mod tests {
    use super::*;

    const SIXTY_FOUR: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn sha256hex_roundtrips_lowercase() {
        let h = Sha256Hex::from_hex(SIXTY_FOUR).unwrap();
        assert_eq!(h.to_hex(), SIXTY_FOUR);
    }

    #[test]
    fn sha256hex_rejects_non_64_hex() {
        // 62 hex chars — wrong length.
        let short = &SIXTY_FOUR[..62];
        assert_eq!(Sha256Hex::from_hex(short), Err(HexError::Length));
    }

    #[test]
    fn sha256hex_rejects_uppercase() {
        let upper = SIXTY_FOUR.to_uppercase();
        assert_eq!(Sha256Hex::from_hex(&upper), Err(HexError::Char));
    }

    #[test]
    fn sha256hex_rejects_short() {
        assert_eq!(Sha256Hex::from_hex("ab"), Err(HexError::Length));
    }

    #[test]
    fn sig_is_64_bytes() {
        let hex = "ab".repeat(64);
        let sig = Sig::from_hex(&hex).unwrap();
        assert_eq!(Sig::LEN, 64);
        assert_eq!(sig.to_hex(), hex);
    }

    #[test]
    fn serde_roundtrip_via_json() {
        let pk = PubKey::from_hex(SIXTY_FOUR).unwrap();
        let json = serde_json::to_string(&pk).unwrap();
        assert_eq!(json, format!("\"{SIXTY_FOUR}\""));
        let back: PubKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pk);
    }

    #[test]
    fn serde_rejects_uppercase_in_json() {
        let json = format!("\"{}\"", SIXTY_FOUR.to_uppercase());
        let res: Result<PubKey, _> = serde_json::from_str(&json);
        assert!(res.is_err());
    }
}
