//! IPC error type. The variants map to the CLI exit codes (Phase 8):
//! 3 not-paired, 4 not-found, 5 daemon-unavailable, 6 denied, 7 offline.

use thiserror::Error;

/// Errors crossing the IPC boundary.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IpcError {
    #[error("daemon unavailable")]
    DaemonUnavailable,
    #[error("not paired")]
    NotPaired,
    #[error("request not found")]
    NotFound,
    #[error("request denied")]
    Denied,
    #[error("offline; cannot complete now")]
    Offline,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not implemented")]
    NotImplemented,
    #[error("io error: {0}")]
    Io(String),
}

/// Result alias for IPC calls.
pub type IpcResult<T> = Result<T, IpcError>;
