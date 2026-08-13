//! The real `org.forgesworn.Charter1` D-Bus client: a zbus proxy over the
//! system bus. `connect()` binds the proxy + bridges the three daemon signals
//! into the [`DaemonEvent`] broadcast channel; a client built with `new()`
//! (unconnected) reports `DaemonUnavailable` so the headless `--features real`
//! build proves the shapes line up without a live bus.
//!
//! The JSON-return parsing is a pure core, verified by the tests below; the
//! zbus connection + signal streams are bus/VM-verified.

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::client::{CharterdClient, ExecProbe, PairingSink};
use crate::dto::{DaemonEvent, Op, RequestStatusView, TimeLeftView};
use crate::dto::{ExecMeta, PairingStateView};
use crate::error::{IpcError, IpcResult};

/// The generated proxy for `org.forgesworn.Charter1`. Method names map to the
/// PascalCase contract members (`submit_request` -> `SubmitRequest`, …); each
/// returns the daemon's JSON string (or bool) which we parse into a DTO.
#[zbus::proxy(
    interface = "org.forgesworn.Charter1",
    default_service = "org.forgesworn.charterd",
    default_path = "/org/forgesworn/charterd"
)]
trait Charter1 {
    async fn submit_request(&self, op: &str, params_json: &str) -> zbus::Result<String>;
    async fn query_status(&self, req_id: &str) -> zbus::Result<String>;
    async fn list_requests(&self, limit: u32) -> zbus::Result<String>;
    async fn cancel_request(&self, req_id: &str) -> zbus::Result<bool>;
    async fn time_left(&self) -> zbus::Result<String>;

    #[zbus(signal)]
    fn request_updated(&self, req_id: &str, status_json: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn time_left_changed(&self, time_left_json: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn lock_state_changed(&self, locked: bool, reason_json: &str) -> zbus::Result<()>;
}

/// Map a zbus error to the IPC error type (a missing daemon is the common case).
fn ipc_err(e: zbus::Error) -> IpcError {
    match e {
        zbus::Error::MethodError(ref name, _, _) if name.as_str().ends_with(".NotFound") => {
            IpcError::NotFound
        }
        _ => IpcError::DaemonUnavailable,
    }
}

/// Parse the daemon's status-list JSON, mapping a parse failure to a clean Io
/// error (never a panic on malformed daemon output).
fn parse_status_list(json: &str) -> IpcResult<Vec<RequestStatusView>> {
    serde_json::from_str(json).map_err(|e| IpcError::Io(format!("bad status json: {e}")))
}

/// Parse one status row (the `QueryStatus(req_id)` single-row form also accepts
/// a one-element array).
fn parse_time_left(json: &str) -> IpcResult<TimeLeftView> {
    serde_json::from_str(json).map_err(|e| IpcError::Io(format!("bad time-left json: {e}")))
}

/// Real D-Bus client. Unconnected until [`connect`](Self::connect); methods on an
/// unconnected client return [`IpcError::DaemonUnavailable`].
pub struct RealCharterdClient {
    proxy: Option<Charter1Proxy<'static>>,
    events: Sender<DaemonEvent>,
}

impl RealCharterdClient {
    /// An unconnected client (methods report `DaemonUnavailable`).
    pub fn new() -> Self {
        let (events, _rx) = broadcast::channel(64);
        RealCharterdClient {
            proxy: None,
            events,
        }
    }

    /// Connect to the system bus, build the proxy, and spawn the signal bridge.
    pub async fn connect() -> IpcResult<Self> {
        let conn = zbus::Connection::system().await.map_err(ipc_err)?;
        let proxy = Charter1Proxy::new(&conn).await.map_err(ipc_err)?;
        let (events, _rx) = broadcast::channel(64);
        spawn_signal_bridge(proxy.clone(), events.clone());
        Ok(RealCharterdClient {
            proxy: Some(proxy),
            events,
        })
    }

    fn proxy(&self) -> IpcResult<&Charter1Proxy<'static>> {
        self.proxy.as_ref().ok_or(IpcError::DaemonUnavailable)
    }
}

impl Default for RealCharterdClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Forward each `org.forgesworn.Charter1` signal into the `DaemonEvent` channel.
fn spawn_signal_bridge(proxy: Charter1Proxy<'static>, tx: Sender<DaemonEvent>) {
    use futures_util::StreamExt as _;

    let p = proxy.clone();
    let t = tx.clone();
    tokio::spawn(async move {
        if let Ok(mut s) = p.receive_request_updated().await {
            while let Some(sig) = s.next().await {
                if let Ok(args) = sig.args() {
                    if let Ok(status) = serde_json::from_str(args.status_json) {
                        let _ = t.send(DaemonEvent::RequestUpdated { status });
                    }
                }
            }
        }
    });

    let p = proxy.clone();
    let t = tx.clone();
    tokio::spawn(async move {
        if let Ok(mut s) = p.receive_time_left_changed().await {
            while let Some(sig) = s.next().await {
                if let Ok(args) = sig.args() {
                    if let Ok(time_left) = serde_json::from_str(args.time_left_json) {
                        let _ = t.send(DaemonEvent::TimeLeftChanged { time_left });
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        if let Ok(mut s) = proxy.receive_lock_state_changed().await {
            while let Some(sig) = s.next().await {
                if let Ok(args) = sig.args() {
                    let reason: Option<String> =
                        serde_json::from_str(args.reason_json).unwrap_or(None);
                    let _ = tx.send(DaemonEvent::LockStateChanged {
                        locked: args.locked,
                        reason,
                    });
                }
            }
        }
    });
}

#[async_trait]
impl CharterdClient for RealCharterdClient {
    async fn submit_request(&self, op: Op, params_json: String) -> IpcResult<String> {
        self.proxy()?
            .submit_request(op.as_wire(), &params_json)
            .await
            .map_err(ipc_err)
    }
    async fn query_status(&self, req_id: &str) -> IpcResult<Vec<RequestStatusView>> {
        let json = self.proxy()?.query_status(req_id).await.map_err(ipc_err)?;
        parse_status_list(&json)
    }
    async fn list_requests(&self, limit: u32) -> IpcResult<Vec<RequestStatusView>> {
        let json = self.proxy()?.list_requests(limit).await.map_err(ipc_err)?;
        parse_status_list(&json)
    }
    async fn cancel_request(&self, req_id: &str) -> IpcResult<bool> {
        self.proxy()?.cancel_request(req_id).await.map_err(ipc_err)
    }
    async fn time_left(&self) -> IpcResult<TimeLeftView> {
        let json = self.proxy()?.time_left().await.map_err(ipc_err)?;
        parse_time_left(&json)
    }
    fn subscribe(&self) -> Receiver<DaemonEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::ReqState;

    #[test]
    fn unconnected_client_reports_daemon_unavailable() {
        let c = RealCharterdClient::new();
        assert!(c.proxy().is_err());
    }

    #[test]
    fn parse_status_list_roundtrips_daemon_json() {
        let rows = vec![RequestStatusView {
            req_id: "r1".into(),
            op: Op::ExecAllow,
            state: ReqState::Pending,
            detail: Some("waiting for approval".into()),
            created_at: 100,
        }];
        let json = serde_json::to_string(&rows).unwrap();
        assert_eq!(parse_status_list(&json).unwrap(), rows);
    }

    #[test]
    fn parse_rejects_malformed_json_cleanly() {
        assert!(matches!(
            parse_status_list("{not json"),
            Err(IpcError::Io(_))
        ));
        assert!(matches!(parse_time_left("nope"), Err(IpcError::Io(_))));
    }

    #[test]
    fn parse_time_left_roundtrips() {
        let t = TimeLeftView::unknown();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(parse_time_left(&json).unwrap(), t);
    }
}

// ---------------------------------------------------------------------------
// Local (non-bus) ports: the exec probe + the read-side pairing view.
// ---------------------------------------------------------------------------

/// Local exec-candidate probe — `stat`s the path for display metadata. It
/// **never hashes** (the content hash never leaves charterd; only the daemon
/// hashes + authorizes by hash), so `ExecMeta` carries no `sha256`.
#[derive(Default)]
pub struct RealExecProbe;

#[async_trait]
impl ExecProbe for RealExecProbe {
    async fn probe(&self, path: &str) -> IpcResult<ExecMeta> {
        let p = std::path::Path::new(path);
        let meta = std::fs::metadata(p).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => IpcError::NotFound,
            _ => IpcError::Io(e.to_string()),
        })?;
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("app")
            .to_string();
        Ok(ExecMeta {
            name,
            size: meta.len(),
            origin: None,
        })
    }
}

/// Read-side pairing view. Reflects the daemon's persisted pairing (best-effort,
/// read-only); the actual `pair` handshake is a **privileged** operation done by
/// `charter-setup` on a provisioned host, so `pair` here reports `NotPaired`
/// (it does not silently fake a pin). Parses the pairing file generically so
/// this contract crate keeps no dependency on the transport's `Pairing` type.
pub struct RealPairingSink {
    pairing_path: PathBuf,
}

impl Default for RealPairingSink {
    fn default() -> Self {
        Self {
            pairing_path: "/var/lib/charter/pairing.json".into(),
        }
    }
}

impl RealPairingSink {
    /// Test/override constructor: redirect the pairing file.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            pairing_path: path.into(),
        }
    }

    fn read_state(&self) -> PairingStateView {
        let guardian = std::fs::read_to_string(&self.pairing_path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("guardian_pubkey")
                    .and_then(|g| g.as_str())
                    .map(str::to_string)
            });
        match guardian {
            Some(g) => PairingStateView {
                paired: true,
                guardian_short: Some(g.chars().take(8).collect()),
            },
            None => PairingStateView {
                paired: false,
                guardian_short: None,
            },
        }
    }
}

#[async_trait]
impl PairingSink for RealPairingSink {
    async fn pair(&self, _bunker_uri: &str) -> IpcResult<PairingStateView> {
        // Pinning writes the daemon's root-owned pairing store — done by the
        // privileged `charter-setup` path on the host, not the unprivileged CLI.
        Err(IpcError::NotPaired)
    }
    async fn pairing_state(&self) -> IpcResult<PairingStateView> {
        Ok(self.read_state())
    }
}

#[cfg(test)]
mod local_port_tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("charter-ipc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn exec_probe_reports_size_and_name_without_hash() {
        let dir = tmp("probe");
        let file = dir.join("SuperTuxKart.AppImage");
        std::fs::write(&file, b"0123456789").unwrap();
        let probe = RealExecProbe;
        let meta = block_on(probe.probe(file.to_str().unwrap())).unwrap();
        assert_eq!(meta.name, "SuperTuxKart");
        assert_eq!(meta.size, 10);
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("sha256").is_none() && v.get("hash").is_none());
    }

    #[test]
    fn exec_probe_missing_path_is_not_found() {
        let probe = RealExecProbe;
        let err = block_on(probe.probe("/no/such/binary")).unwrap_err();
        assert!(matches!(err, IpcError::NotFound));
    }

    #[test]
    fn pairing_state_reflects_the_persisted_file() {
        let dir = tmp("pair");
        let path = dir.join("pairing.json");
        let sink = RealPairingSink::with_path(&path);
        // Absent file -> not paired.
        assert!(!block_on(sink.pairing_state()).unwrap().paired);
        // Present file -> paired + short guardian fingerprint.
        std::fs::write(
            &path,
            format!(r#"{{"guardian_pubkey":"{}"}}"#, "ab".repeat(32)),
        )
        .unwrap();
        let st = block_on(sink.pairing_state()).unwrap();
        assert!(st.paired);
        assert_eq!(st.guardian_short.as_deref(), Some("abababab"));
    }
}
