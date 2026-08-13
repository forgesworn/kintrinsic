//! Proto (de)serialization errors.

use std::fmt;

/// Errors parsing a wire payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// JSON did not parse / did not match the payload shape.
    Json(String),
    /// Unsupported schema version.
    BadVersion(u32),
    /// Op-specific params failed to parse.
    BadParams(String),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Json(e) => write!(f, "json: {e}"),
            ProtoError::BadVersion(v) => write!(f, "unsupported version: {v}"),
            ProtoError::BadParams(e) => write!(f, "bad params: {e}"),
        }
    }
}

impl std::error::Error for ProtoError {}
