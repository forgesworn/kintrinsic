//! The live `org.forgesworn.Charter1` zbus server (the `real` feature). Exposes
//! exactly the [`contract`](charter_ipc::contract) method/signal allowlist,
//! backed by the broker + enforcer; the structural allowlist + caller-auth
//! invariants are pinned (without a bus) in [`crate::dbus_surface`].
//!
//! No-local-authority holds at the surface: the only effect-producing method is
//! `SubmitRequest` (publish-only) — there is no enact/approve/grant method. The
//! record/time-left -> DTO mappings are pure + unit-tested; the live bus serve
//! is VM-verified.

#![cfg(feature = "real")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use charter_ipc::contract::{BUS_NAME, INTERFACE, OBJECT_PATH};
use charter_ipc::dto::{Op, ReqState, RequestStatusView, TimeLeftView};
use charter_proto::OpType;
use charter_schedule::enforcer::Remaining;
use charter_sys::effects::RealApprovedExecStore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use zbus::SignalContext;

use crate::device_limits::{home_for_uid, user_for_uid};
use crate::enactors::exec_allow::plan_exec_allow;
use crate::exec_guard::{FileKind, ProbeExec};
use crate::lifecycle::{RequestRecord, RequestState};
use crate::ports::EventSink;
use crate::runtime::RealBroker;

/// Per-uid time-left snapshots the enforcement loop publishes each tick and the
/// `TimeLeft` method serves to the **calling** child (each child on a shared box
/// sees their own). A snapshot map (rather than sharing the live
/// `MultiChildEnforcer`) keeps the status method off the hot enforce loop's lock;
/// a snapshot is at most one tick (~seconds) stale.
pub type TimeLeftSnapshots = Arc<Mutex<BTreeMap<u32, TimeLeftView>>>;

// ---- pure DTO mappings (unit-tested) --------------------------------------

/// Parse a dotted op wire string into an [`OpType`].
pub fn op_from_wire(op: &str) -> Option<OpType> {
    match op {
        "install.flatpak" => Some(OpType::InstallFlatpak),
        "exec.allow" => Some(OpType::ExecAllow),
        "time.extend" => Some(OpType::TimeExtend),
        // The ward's ask to open (or hold open) an `askFirst` app. Answering
        // it (Decision + clause re-sign) is guardian-side machinery this
        // surface does not implement yet — but the ward's own SUBMIT must
        // work today, exactly like every other brokered ask.
        "app.open" => Some(OpType::AppOpen),
        _ => None,
    }
}

/// Map the internal [`OpType`] to the IPC DTO [`Op`].
pub fn op_to_ipc(op: OpType) -> Op {
    match op {
        OpType::InstallFlatpak => Op::InstallFlatpak,
        OpType::InstallApk => Op::InstallApk,
        OpType::ExecAllow => Op::ExecAllow,
        OpType::TimeExtend => Op::TimeExtend,
        OpType::AppOpen => Op::AppOpen,
    }
}

/// Map the lifecycle [`RequestState`] to the IPC DTO [`ReqState`].
pub fn state_to_ipc(state: RequestState) -> ReqState {
    match state {
        RequestState::Pending => ReqState::Pending,
        RequestState::Enacting => ReqState::Enacting,
        RequestState::Enacted => ReqState::Enacted,
        RequestState::Denied => ReqState::Denied,
        RequestState::Failed => ReqState::Failed,
        RequestState::Rejected => ReqState::Rejected,
        RequestState::Expired => ReqState::Expired,
        RequestState::Cancelled => ReqState::Cancelled,
    }
}

/// Project a broker [`RequestRecord`] onto the user-surface [`RequestStatusView`].
pub fn record_to_view(rec: &RequestRecord) -> RequestStatusView {
    RequestStatusView {
        req_id: rec.req_id.to_hex(),
        op: op_to_ipc(rec.op),
        state: state_to_ipc(rec.state),
        detail: rec.detail.clone(),
        created_at: rec.created_at,
    }
}

/// Map the enforcer [`Remaining`] to the [`TimeLeftView`] DTO. `now` turns the
/// "seconds until next window" into the absolute unix time the DTO carries.
pub fn remaining_to_view(r: &Remaining, now: i64) -> TimeLeftView {
    TimeLeftView {
        effective_seconds: r.effective_secs,
        schedule_seconds: r.schedule_secs,
        budget_seconds: r.budget_secs,
        extension_seconds: r.extension_secs,
        locked: r.locked,
        reason: r.reason.map(|x| format!("{x:?}").to_lowercase()),
        next_open: r.next_open_secs.map(|s| (now + s).max(0) as u64),
        // Served locally from fresh state — not a stale offline cache.
        offline: false,
        // Stamped by the loop when a learning clause is in force.
        learning_today_seconds: None,
        used_today_seconds: None,
        buckets: Vec::new(),
        // Which cap is which, so a ward on both a daily and a weekly limit can
        // be told the one that is about to stop them.
        budget_day_seconds: r.budget_day_secs,
        budget_week_seconds: r.budget_week_secs,
        // Stamped by the loop from the child's `apps` clause, same as buckets.
        ask_first: Vec::new(),
    }
}

// ---- event sink -> signal bridge ------------------------------------------

/// An [`EventSink`] that forwards request-id updates to the signal pump over an
/// unbounded channel (a sync sink cannot itself emit an async D-Bus signal).
pub struct ChannelEventSink {
    tx: UnboundedSender<String>,
}

impl ChannelEventSink {
    pub fn new(tx: UnboundedSender<String>) -> Self {
        Self { tx }
    }
}

impl EventSink for ChannelEventSink {
    fn request_updated(&self, req_id: &str, _state: RequestState) {
        let _ = self.tx.send(req_id.to_string());
    }
    fn lock_state_changed(&self, _locked: bool, _reason: Option<String>) {
        // LockStateChanged is emitted from the enforcement loop (it owns the
        // freeze/lock transitions); the signal is declared on the interface.
    }
}

/// Drain request-id updates and emit `RequestUpdated` with the freshest status.
pub async fn signal_pump(
    conn: zbus::Connection,
    broker: Arc<RealBroker>,
    mut rx: UnboundedReceiver<String>,
) {
    let emitter = match SignalContext::new(&conn, OBJECT_PATH) {
        Ok(e) => e,
        Err(_) => return,
    };
    while let Some(req_id) = rx.recv().await {
        if let Some(rec) = broker.status(&req_id).into_iter().next() {
            if let Ok(json) = serde_json::to_string(&record_to_view(&rec)) {
                let _ = CharterInterface::request_updated(&emitter, &req_id, &json).await;
            }
        }
    }
}

// ---- the served interface --------------------------------------------------

/// The object served at `/org/forgesworn/charterd`. The broker is present only
/// when the device is paired to a phone guardian; the time-left/status surface is
/// served in **both** modes (device-only children still check their time), while
/// the guardian-brokered methods (ask-for-more / install) are gracefully
/// unavailable until pairing.
pub struct CharterInterface {
    broker: Option<Arc<RealBroker>>,
    snapshots: TimeLeftSnapshots,
}

impl CharterInterface {
    /// The broker, or a normie-friendly "not paired yet" error for the guardian-
    /// brokered methods (there is no one to approve without a phone guardian).
    fn require_broker(&self) -> zbus::fdo::Result<&Arc<RealBroker>> {
        self.broker.as_ref().ok_or_else(|| {
            zbus::fdo::Error::Failed(
                "Kintrinsic isn't connected to a phone guardian yet — ask your parent to finish \
                 pairing (\"Kintrinsic — Pair Guardian\") to send this request."
                    .into(),
            )
        })
    }
}

/// The Unix uid of the D-Bus caller, resolved from the bus daemon's connection
/// credentials — **unspoofable** (a client cannot claim another uid). `None`
/// when it can't be determined, which `time_left` fail-safes to an "unknown"
/// readout rather than leaking or guessing another child's time.
async fn caller_uid(conn: &zbus::Connection, hdr: &zbus::message::Header<'_>) -> Option<u32> {
    let sender = hdr.sender()?.to_owned();
    let proxy = zbus::fdo::DBusProxy::new(conn).await.ok()?;
    proxy
        .get_connection_credentials(sender.into())
        .await
        .ok()?
        .unix_user_id()
}

/// The live [`ProbeExec`] backing the `exec.allow` confused-deputy guard: the
/// privileged, uid-scoped read + file-type probe. charterd runs as **root**, so
/// a naive `access(R_OK)` would ALWAYS pass and defeat the guard — the
/// readability question must be answered by the KERNEL *as the calling child*
/// (their real uid + supplementary groups). Everything here fails **closed**: any
/// resolution/spawn failure reads as "not readable" / "special file", so a
/// candidate is refused rather than admitted. Exercised on real hardware (the
/// request-building path is unit-tested against fakes).
struct RealProbeExec;

impl ProbeExec for RealProbeExec {
    fn file_kind(&self, path: &str) -> FileKind {
        use std::os::unix::fs::FileTypeExt;
        // lstat (does NOT follow a final symlink): a symlink pointing out of the
        // managed tree must be caught here, never silently resolved as root.
        match std::fs::symlink_metadata(path) {
            Ok(md) => {
                let t = md.file_type();
                if t.is_symlink() {
                    FileKind::Symlink
                } else if t.is_dir() {
                    FileKind::Dir
                } else if t.is_fifo() {
                    FileKind::Fifo
                } else if t.is_block_device() || t.is_char_device() {
                    FileKind::Device
                } else if t.is_file() {
                    FileKind::Regular
                } else {
                    // Sockets / anything exotic: non-regular -> refused.
                    FileKind::Device
                }
            }
            Err(_) => FileKind::Missing,
        }
    }

    fn can_read_as_uid(&self, path: &str, uid: u32) -> bool {
        // Drop to the calling child (their uid + initgroups, via `runuser`) and
        // let the kernel answer "can THEY read it?" — honoring ownership, group,
        // and ACLs exactly. `timeout` bounds a pathological stat (e.g. a stuck
        // mount) so this can never wedge the D-Bus serve task.
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let Some(user) = user_for_uid(&passwd, uid) else {
            return false; // unknown uid -> fail-closed
        };
        std::process::Command::new("timeout")
            .arg("5")
            .arg("runuser")
            .arg("-u")
            .arg(&user)
            .arg("--")
            .arg("test")
            .arg("-r")
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[zbus::interface(name = "org.forgesworn.Charter1")]
impl CharterInterface {
    /// Build + publish a REQUEST. Enacts nothing. Returns the reqId hex.
    ///
    /// `exec.allow` is an exec-ADMISSION path and is special-cased: charterd
    /// (root) must NOT trust the client's raw `{"path": ...}`. It resolves the
    /// KERNEL-attested caller uid (SO_PEERCRED via the bus — never the params),
    /// runs the confused-deputy guard against THAT caller's own home, inspects
    /// the bytes server-side (sha256 + size), and binds the REAL source path to
    /// the reqId so the enactor can relocate + re-hash it. (Previously it passed
    /// the raw path as params with `source_path = None`, so every real grant died
    /// with "no source path bound to reqId".) All other ops publish verbatim with
    /// no bound source path.
    async fn submit_request(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        op: &str,
        params_json: &str,
    ) -> zbus::fdo::Result<String> {
        let op = op_from_wire(op)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("unknown op: {op}")))?;
        let params: serde_json::Value = serde_json::from_str(params_json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(format!("params: {e}")))?;
        // Fail early (before any guard work) with the friendly unpaired message.
        let broker = self.require_broker()?;

        let (params, source_path) = if op == OpType::ExecAllow {
            // The caller uid comes from the KERNEL, never the request body.
            let uid = caller_uid(conn, &hdr).await.ok_or_else(|| {
                zbus::fdo::Error::Failed("couldn't identify the requesting user".into())
            })?;
            // A caller may only admit paths under THEIR OWN managed tree.
            let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
            let managed_root = home_for_uid(&passwd, uid).ok_or_else(|| {
                zbus::fdo::Error::Failed("no home directory for the requesting user".into())
            })?;
            // Guard + inspect. A rejection (confused-deputy / unreadable / not a
            // regular file / bad hash) fails the call — no broken request is
            // published.
            let (params, path) = plan_exec_allow(
                &params,
                uid,
                &RealApprovedExecStore::default(),
                &managed_root,
                &RealProbeExec,
            )
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            (params, Some(path))
        } else {
            (params, None)
        };

        // Record WHO asked (S8) — kernel-attested, never from the body. On a
        // shared family box this is what lets one child's asks be theirs.
        // `None` (an unidentifiable caller) makes the record root-only, which
        // is the fail-closed direction: the request still travels and can
        // still be granted, it just is not listed to a non-root reader.
        let caller = caller_uid(conn, &hdr).await;
        let req_id = broker
            .submit_as(op, params, source_path, caller)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(req_id.to_hex())
    }

    /// Status for one reqId (empty = all), newest first, as a JSON array.
    /// **The CALLER's own requests only** (root sees all) — see [S8] on
    /// `list_requests`. Empty when unpaired (there are no guardian-brokered
    /// requests device-only).
    async fn query_status(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        req_id: &str,
    ) -> zbus::fdo::Result<String> {
        let uid = caller_uid(conn, &hdr).await;
        let views: Vec<_> = match (&self.broker, uid) {
            (Some(b), Some(uid)) => b
                .status_for(uid, req_id)
                .iter()
                .map(record_to_view)
                .collect(),
            // Fail closed on an unidentifiable caller: show nothing rather
            // than everything (same posture as `time_left`'s "unknown").
            _ => Vec::new(),
        };
        serde_json::to_string(&views).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    /// Newest-first list bounded by `limit` (0 = unbounded), as a JSON array.
    /// Empty when unpaired.
    ///
    /// **Scoped to the calling uid** (S8, review 2026-08-07), kernel-attested
    /// via `GetConnectionCredentials` — root exempt. This used to hand any
    /// local account every account's pending asks: what each child had asked
    /// for and when, on a shared family laptop, which is one sibling's
    /// business and not the other's. It also handed out the req_ids that were
    /// the input to `CancelRequest`.
    async fn list_requests(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        limit: u32,
    ) -> zbus::fdo::Result<String> {
        let uid = caller_uid(conn, &hdr).await;
        let views: Vec<_> = match (&self.broker, uid) {
            (Some(b), Some(uid)) => b
                .list_for(uid, limit as usize)
                .iter()
                .map(record_to_view)
                .collect(),
            _ => Vec::new(),
        };
        serde_json::to_string(&views).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    /// Withdraw a pending request. Returns true if it was pending (always false
    /// when unpaired — no brokered requests exist).
    ///
    /// **Only your own** (S8) — root exempt. Withdrawing a sibling's ask before
    /// a parent ever sees it is a small cruelty that no local account should be
    /// able to perform, and req_ids were enumerable through `ListRequests`.
    /// "Not yours" and "no such request" both return false, so a refusal never
    /// confirms that a reqId is real.
    async fn cancel_request(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        req_id: &str,
    ) -> bool {
        let Some(uid) = caller_uid(conn, &hdr).await else {
            return false;
        };
        self.broker
            .as_ref()
            .is_some_and(|b| b.cancel_as(uid, req_id))
    }

    /// The **calling** child's own time-left breakdown JSON. Each child on a
    /// shared family box sees THEIR remaining time; fail-safe to `unknown` (never
    /// "unlimited") when the caller can't be identified or has no snapshot yet.
    /// The per-uid snapshots are published every tick by the enforcement loop.
    async fn time_left(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<String> {
        let view = match caller_uid(conn, &hdr).await {
            Some(uid) => self
                .snapshots
                .lock()
                .expect("snapshots lock")
                .get(&uid)
                .cloned()
                .unwrap_or_else(TimeLeftView::unknown),
            None => TimeLeftView::unknown(),
        };
        serde_json::to_string(&view).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    // The three contract signals (declared here so introspection equals the
    // allowlist). RequestUpdated is emitted by the signal pump; LockStateChanged
    // + TimeLeftChanged are emitted from the enforcement loop.
    #[zbus(signal)]
    async fn request_updated(
        emitter: &SignalContext<'_>,
        req_id: &str,
        status_json: &str,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn time_left_changed(
        emitter: &SignalContext<'_>,
        time_left_json: &str,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn lock_state_changed(
        emitter: &SignalContext<'_>,
        locked: bool,
        reason_json: &str,
    ) -> zbus::Result<()>;
}

/// Request the well-known name + serve the interface on the system bus. Served in
/// both paired and device-only modes; `broker` is `None` when unpaired.
pub async fn serve(
    broker: Option<Arc<RealBroker>>,
    snapshots: TimeLeftSnapshots,
) -> zbus::Result<zbus::Connection> {
    let iface = CharterInterface { broker, snapshots };
    zbus::connection::Builder::system()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, iface)?
        .build()
        .await
}

/// The served interface name (kept in sync with the contract by construction).
pub const SERVED_INTERFACE: &str = INTERFACE;
