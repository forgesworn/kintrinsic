//! The system-layer error type.

use thiserror::Error;

/// Errors from any `charter-sys` port. `Real*` stubs return
/// [`SysError::NotImplemented`] / [`SysError::Unsupported`] in the headless
/// gate (they are only ever run on a real machine).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SysError {
    #[error("not implemented")]
    NotImplemented,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("not found")]
    NotFound,
    #[error("io error: {0}")]
    Io(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    /// A monotonicity / rollback conflict (e.g. a clause with a stale issuedAt).
    #[error("conflict: {0}")]
    Conflict(String),
}

/// Result alias for system-layer ports.
pub type SysResult<T> = Result<T, SysError>;
