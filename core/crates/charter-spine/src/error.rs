//! charterd error types.

use thiserror::Error;

/// Errors enacting a verified grant.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EnactError {
    /// No enactor is registered for this op (fail-closed, never "allow").
    #[error("no enactor for op")]
    NoEnactor,
    /// A terminal failure — the grant cannot succeed (re-request needed).
    #[error("terminal: {0}")]
    Terminal(String),
    /// A transient failure — the in-memory verified grant may be retried.
    #[error("transient: {0}")]
    Transient(String),
}

impl EnactError {
    /// Whether the failure is terminal (no point retrying the in-memory grant).
    pub fn is_terminal(&self) -> bool {
        matches!(self, EnactError::NoEnactor | EnactError::Terminal(_))
    }
}

/// Top-level broker errors.
#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not paired")]
    NotPaired,
    #[error("system error: {0}")]
    Sys(#[from] charter_sys::SysError),
    #[error("broker stopped")]
    Stopped,
}
