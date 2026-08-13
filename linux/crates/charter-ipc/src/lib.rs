//! `charter-ipc` — **THE** D-Bus contract for Kintrinsic for Linux: the frozen
//! bus/path/interface + method/signal names ([`contract`]), the JSON DTOs
//! ([`dto`]), the IPC port traits ([`client`]), an in-memory `mock`, and a
//! compile-only `real` client. Built early so `charterd`, `charter-cli`, the
//! GUI core, the shipped D-Bus policy, and the VM introspection assertion all
//! consume the same constants.

pub mod client;
pub mod contract;
pub mod dto;
pub mod error;

#[cfg(feature = "mock")]
pub mod mock;

#[cfg(feature = "real")]
pub mod real;

pub use client::{CharterdClient, ExecProbe, PairingSink};
pub use dto::{
    DaemonEvent, ExecMeta, Op, PairingStateView, ReqState, RequestStatusView, TimeLeftView,
};
pub use error::{IpcError, IpcResult};
