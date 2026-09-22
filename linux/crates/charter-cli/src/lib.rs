//! `charter_cli` — the unprivileged CLI logic: pure command dispatch over the
//! `charter-ipc` ports. Every effect goes through `SubmitRequest` (publish-
//! only); there is no enact/approve path. The bin (`charter`) is a thin wrapper
//! over [`dispatch`].

pub mod dispatch;
pub mod render;

pub use dispatch::{
    dispatch, exit_code, CliOutcome, EXIT_DAEMON_UNAVAILABLE, EXIT_DENIED, EXIT_NOT_FOUND,
    EXIT_NOT_PAIRED, EXIT_OFFLINE, EXIT_OK, EXIT_USAGE,
};
