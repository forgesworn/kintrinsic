//! The IPC port traits the user surface (CLI, GUI core) talks to. The daemon
//! implements them over D-Bus; tests implement them in-memory.

use async_trait::async_trait;
use tokio::sync::broadcast::Receiver;

use crate::dto::{DaemonEvent, ExecMeta, Op, PairingStateView, RequestStatusView, TimeLeftView};
use crate::error::IpcResult;

/// The brokered-request client surface — the `org.forgesworn.Charter1` methods.
///
/// **No-local-authority invariant:** there is no `enact`/`approve`/`install`
/// method here. The only effect-producing call is [`submit_request`], which
/// merely publishes a REQUEST; a brokered effect happens only via a verified
/// grant inside the daemon.
///
/// [`submit_request`]: CharterdClient::submit_request
#[async_trait]
pub trait CharterdClient: Send + Sync {
    /// Build + publish a REQUEST. Returns the `req_id`. Enacts nothing.
    async fn submit_request(&self, op: Op, params_json: String) -> IpcResult<String>;

    /// Status for one `req_id`, or all when empty.
    async fn query_status(&self, req_id: &str) -> IpcResult<Vec<RequestStatusView>>;

    /// Newest-first list, bounded by `limit` (0 = unbounded).
    async fn list_requests(&self, limit: u32) -> IpcResult<Vec<RequestStatusView>>;

    /// Withdraw a pending request you own. Returns true if it was pending.
    async fn cancel_request(&self, req_id: &str) -> IpcResult<bool>;

    /// The current time-left breakdown.
    async fn time_left(&self) -> IpcResult<TimeLeftView>;

    /// Subscribe to the daemon event stream (signal bridge). Each call yields
    /// an independent receiver.
    fn subscribe(&self) -> Receiver<DaemonEvent>;
}

/// Pairing port — used by the first-run wizard / `charter pair`. Pinning a
/// guardian is admin-only at the OS layer (Phase 9); the port itself just
/// carries the `bunker://` URI to the privileged setup component.
#[async_trait]
pub trait PairingSink: Send + Sync {
    /// Pin a guardian from a `bunker://` URI.
    async fn pair(&self, bunker_uri: &str) -> IpcResult<PairingStateView>;
    /// Current pairing state.
    async fn pairing_state(&self) -> IpcResult<PairingStateView>;
}

/// Exec-probe port — used by `charter run <path>` / the AppImage MIME handler
/// to surface metadata before submitting an `exec.allow` request. Never
/// returns a hash (the hash never leaves the daemon).
#[async_trait]
pub trait ExecProbe: Send + Sync {
    /// Probe a candidate path for display metadata.
    async fn probe(&self, path: &str) -> IpcResult<ExecMeta>;
}
