//! In-memory mock implementations of the IPC ports for headless tests. The
//! `MockCharterdClient` carries a scriptable guardian decision and a settable
//! TimeLeft state so CLI/GUI tests can drive every branch deterministically.

use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::client::{CharterdClient, ExecProbe, PairingSink};
use crate::dto::{
    DaemonEvent, ExecMeta, Op, PairingStateView, ReqState, RequestStatusView, TimeLeftView,
};
use crate::error::{IpcError, IpcResult};

/// How the scripted guardian responds to each submitted request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedDecision {
    /// Leave the request Pending (realistic async — no immediate grant).
    LeavePending,
    /// Auto-approve: advance to Enacted and emit an update.
    Allow,
    /// Auto-deny.
    Deny,
}

struct State {
    next_id: u64,
    requests: Vec<RequestStatusView>,
    /// The exact (op, params_json) of every submitted request — RequestStatusView
    /// does not carry params, so tests assert the wire payload here.
    submitted: Vec<(Op, String)>,
    decision: ScriptedDecision,
    time_left: TimeLeftView,
    paired: bool,
    clock: u64,
}

/// A scriptable in-memory `CharterdClient`.
pub struct MockCharterdClient {
    state: Mutex<State>,
    events: Sender<DaemonEvent>,
}

impl MockCharterdClient {
    /// A paired client that leaves requests pending and reports unlimited time.
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(64);
        MockCharterdClient {
            state: Mutex::new(State {
                next_id: 1,
                requests: Vec::new(),
                submitted: Vec::new(),
                decision: ScriptedDecision::LeavePending,
                time_left: TimeLeftView {
                    effective_seconds: -1,
                    schedule_seconds: -1,
                    budget_seconds: -1,
                    extension_seconds: 0,
                    locked: false,
                    reason: None,
                    next_open: None,
                    offline: false,
                    learning_today_seconds: None,

                    used_today_seconds: None,

                    buckets: Vec::new(),

                    budget_day_seconds: -1,

                    budget_week_seconds: -1,

                    ask_first: Vec::new(),
                },
                paired: true,
                clock: 1_700_000_000,
            }),
            events,
        }
    }

    /// Set the scripted guardian decision applied to subsequent submits.
    pub fn with_decision(self, d: ScriptedDecision) -> Self {
        self.state.lock().expect("lock").decision = d;
        self
    }

    /// Set the reported pairing state.
    pub fn with_paired(self, paired: bool) -> Self {
        self.state.lock().expect("lock").paired = paired;
        self
    }

    /// Set the reported TimeLeft breakdown.
    pub fn with_time_left(self, t: TimeLeftView) -> Self {
        self.state.lock().expect("lock").time_left = t;
        self
    }

    /// The (op, params_json) of every request submitted so far.
    pub fn submitted(&self) -> Vec<(Op, String)> {
        self.state.lock().expect("lock").submitted.clone()
    }

    fn emit(events: &Sender<DaemonEvent>, ev: DaemonEvent) {
        // Ignore send errors (no subscribers).
        let _ = events.send(ev);
    }
}

impl Default for MockCharterdClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CharterdClient for MockCharterdClient {
    async fn submit_request(&self, op: Op, params_json: String) -> IpcResult<String> {
        // Validate the params JSON shape (object) — fail-closed on garbage.
        let parsed: serde_json::Value = serde_json::from_str(&params_json)
            .map_err(|e| IpcError::Invalid(format!("params not JSON: {e}")))?;
        if !parsed.is_object() {
            return Err(IpcError::Invalid("params must be a JSON object".into()));
        }

        let mut st = self.state.lock().expect("lock");
        if !st.paired {
            return Err(IpcError::NotPaired);
        }
        st.submitted.push((op, params_json.clone()));
        let id = format!("{:064x}", st.next_id);
        st.next_id += 1;
        let created_at = st.clock;
        let mut view = RequestStatusView {
            req_id: id.clone(),
            op,
            state: ReqState::Pending,
            detail: Some("waiting for approval".into()),
            created_at,
        };
        match st.decision {
            ScriptedDecision::LeavePending => {}
            ScriptedDecision::Allow => {
                view.state = ReqState::Enacted;
                view.detail = Some("approved".into());
            }
            ScriptedDecision::Deny => {
                view.state = ReqState::Denied;
                view.detail = Some("not approved".into());
            }
        }
        st.requests.push(view.clone());
        drop(st);
        Self::emit(&self.events, DaemonEvent::RequestUpdated { status: view });
        Ok(id)
    }

    async fn query_status(&self, req_id: &str) -> IpcResult<Vec<RequestStatusView>> {
        let st = self.state.lock().expect("lock");
        if req_id.is_empty() {
            return Ok(st.requests.clone());
        }
        let hits: Vec<_> = st
            .requests
            .iter()
            .filter(|r| r.req_id == req_id)
            .cloned()
            .collect();
        if hits.is_empty() {
            Err(IpcError::NotFound)
        } else {
            Ok(hits)
        }
    }

    async fn list_requests(&self, limit: u32) -> IpcResult<Vec<RequestStatusView>> {
        let st = self.state.lock().expect("lock");
        let mut rows = st.requests.clone();
        rows.reverse(); // newest first
        if limit > 0 {
            rows.truncate(limit as usize);
        }
        Ok(rows)
    }

    async fn cancel_request(&self, req_id: &str) -> IpcResult<bool> {
        let mut st = self.state.lock().expect("lock");
        let mut cancelled = false;
        let mut updated: Option<RequestStatusView> = None;
        for r in st.requests.iter_mut() {
            if r.req_id == req_id && r.state == ReqState::Pending {
                r.state = ReqState::Cancelled;
                r.detail = Some("cancelled".into());
                cancelled = true;
                updated = Some(r.clone());
                break;
            }
        }
        drop(st);
        if let Some(v) = updated {
            Self::emit(&self.events, DaemonEvent::RequestUpdated { status: v });
        }
        Ok(cancelled)
    }

    async fn time_left(&self) -> IpcResult<TimeLeftView> {
        Ok(self.state.lock().expect("lock").time_left.clone())
    }

    fn subscribe(&self) -> Receiver<DaemonEvent> {
        self.events.subscribe()
    }
}

/// A simple mock pairing sink.
pub struct MockPairingSink {
    state: Mutex<PairingStateView>,
}

impl MockPairingSink {
    /// Construct with an initial pairing state.
    pub fn new(paired: bool) -> Self {
        MockPairingSink {
            state: Mutex::new(PairingStateView {
                paired,
                guardian_short: paired.then(|| "npub1guardian".to_string()),
            }),
        }
    }
}

#[async_trait]
impl PairingSink for MockPairingSink {
    async fn pair(&self, bunker_uri: &str) -> IpcResult<PairingStateView> {
        let mut st = self.state.lock().expect("lock");
        if st.paired {
            return Err(IpcError::Invalid("already paired".into()));
        }
        if !bunker_uri.starts_with("bunker://") {
            return Err(IpcError::Invalid("not a bunker:// uri".into()));
        }
        st.paired = true;
        st.guardian_short = Some("npub1guardian".into());
        Ok(st.clone())
    }

    async fn pairing_state(&self) -> IpcResult<PairingStateView> {
        Ok(self.state.lock().expect("lock").clone())
    }
}

/// A mock exec probe: returns metadata for known paths, NotFound otherwise.
pub struct MockExecProbe {
    known: Mutex<Vec<(String, ExecMeta)>>,
}

impl MockExecProbe {
    /// Empty probe.
    pub fn new() -> Self {
        MockExecProbe {
            known: Mutex::new(Vec::new()),
        }
    }

    /// Register a path -> metadata mapping.
    pub fn with_path(self, path: &str, meta: ExecMeta) -> Self {
        self.known
            .lock()
            .expect("lock")
            .push((path.to_string(), meta));
        self
    }
}

impl Default for MockExecProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecProbe for MockExecProbe {
    async fn probe(&self, path: &str) -> IpcResult<ExecMeta> {
        self.known
            .lock()
            .expect("lock")
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, m)| m.clone())
            .ok_or(IpcError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_returns_id_and_records_pending() {
        let c = MockCharterdClient::new();
        let id = c
            .submit_request(Op::InstallFlatpak, "{\"ref\":\"org.x.Y\"}".into())
            .await
            .unwrap();
        assert_eq!(id.len(), 64);
        let rows = c.query_status(&id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, ReqState::Pending);
    }

    #[tokio::test]
    async fn submit_rejects_non_object_params() {
        let c = MockCharterdClient::new();
        let err = c
            .submit_request(Op::TimeExtend, "42".into())
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::Invalid(_)));
    }

    #[tokio::test]
    async fn unpaired_submit_is_not_paired() {
        let c = MockCharterdClient::new().with_paired(false);
        let err = c
            .submit_request(Op::InstallFlatpak, "{}".into())
            .await
            .unwrap_err();
        assert_eq!(err, IpcError::NotPaired);
    }

    #[tokio::test]
    async fn unknown_status_is_not_found() {
        let c = MockCharterdClient::new();
        assert_eq!(
            c.query_status("deadbeef").await.unwrap_err(),
            IpcError::NotFound
        );
    }

    #[tokio::test]
    async fn scripted_allow_enacts() {
        let c = MockCharterdClient::new().with_decision(ScriptedDecision::Allow);
        let id = c
            .submit_request(Op::InstallFlatpak, "{}".into())
            .await
            .unwrap();
        let rows = c.query_status(&id).await.unwrap();
        assert_eq!(rows[0].state, ReqState::Enacted);
    }

    #[tokio::test]
    async fn subscribe_receives_request_events() {
        let c = MockCharterdClient::new().with_decision(ScriptedDecision::Allow);
        let mut rx = c.subscribe();
        c.submit_request(Op::InstallFlatpak, "{}".into())
            .await
            .unwrap();
        let ev = rx.recv().await.unwrap();
        matches!(ev, DaemonEvent::RequestUpdated { .. });
    }

    #[tokio::test]
    async fn cancel_pending_returns_true() {
        let c = MockCharterdClient::new();
        let id = c
            .submit_request(Op::InstallFlatpak, "{}".into())
            .await
            .unwrap();
        assert!(c.cancel_request(&id).await.unwrap());
        assert!(!c.cancel_request(&id).await.unwrap()); // already cancelled
    }
}
