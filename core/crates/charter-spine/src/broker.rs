//! The broker: submit -> publish, and the receive pipeline (verify -> reduce ->
//! enact) for grants, plus clause authentication. Single-use is race-free via
//! the atomic `check_and_consume` (consume happens *inside* `verify_grant`,
//! before any enact). In production a single task owns the broker; here it is
//! internally synchronized so the headless tests drive it directly.

use std::collections::BTreeMap;
use std::sync::Mutex;

use charter_primitives::{Nonce, PubKey, ReqId};
use charter_proto::{
    ClauseKind, ClausePayload, GrantPayload, OpType, RequestPayload, UsageSyncPayload,
    USAGE_SYNC_STORE_KEY,
};
use charter_sys::persistence::{ChildClauseStore, ClauseStore, PairingStore, PendingStore};
use charter_sys::{Clock, SystemLayer};
use charter_transport::{Entropy, ReceivedClause, ReceivedGrant};
use charter_verify::{verify_clause, verify_grant, verify_usage_sync, VerifyParams};

use crate::enactor::{EnactContext, EnactorRegistry};
use crate::error::BrokerError;
use crate::lifecycle::{transition, Effect, Event, RequestRecord, RequestState};
use crate::ports::EventSink;
use crate::transport_facade::TransportFacade;

struct State {
    requests: BTreeMap<String, RequestRecord>,
    cursor: u64,
}

/// How far BEHIND wall-clock the next poll's `since` cursor sits. A guardian
/// gift-wrap's `created_at` may legitimately trail the device's clock by up to
/// the transport's NIP-59 jitter tolerance (clock skew + store-and-forward
/// backdating), and a lossy relay may redeliver across poll boundaries.
/// Advancing the cursor to bare wall-clock `now` lets the relay's `since` filter
/// **permanently** exclude any such event (its `created_at` < `now`) until the
/// daemon restarts. Keeping the cursor a full jitter window behind now makes the
/// relay re-offer anything the transport would still accept. Reprocessing is
/// side-effect-free: a clause re-verifies and is rejected by the per-(subject,
/// kind) monotonic `issuedAt` floor, and a grant is gated by its reqId/Pending
/// state + single-use `consumed_ids`.
const POLL_LOOKBACK_SECS: u64 = charter_transport::nip59::MAX_JITTER_SECS;

/// Cap on retained **terminal** request records in the persisted `PendingStore`.
/// Every brokered request eventually reaches a terminal state (Enacted / Denied /
/// Failed / Rejected / Expired / Cancelled) but was previously never removed, so
/// the on-disk store grew without bound over a device's lifetime. We keep the
/// most-recent `MAX_TERMINAL_RECORDS` terminal records (by `created_at`) and evict
/// the oldest beyond that. **Non-terminal records (Pending / Enacting) are never
/// evicted** — they carry in-flight guardian decisions and the boot-resubscribe
/// set, so dropping one would silently strand a request. This is a generous
/// history window for a family device; the number is a durability bound, not a
/// UX limit.
const MAX_TERMINAL_RECORDS: usize = 256;

/// Evict the oldest **terminal** records beyond `max` from both the in-memory
/// map and the persisted store, oldest-first by `created_at` (ties broken by the
/// record key for determinism). Non-terminal (Pending / Enacting) records are
/// never counted and never removed. Pure over its inputs — no `Broker` needed —
/// so the eviction bound is unit-testable directly.
fn prune_terminal_records(
    requests: &mut BTreeMap<String, RequestRecord>,
    store: &dyn PendingStore,
    max: usize,
) {
    let mut terminal: Vec<(String, u64)> = requests
        .iter()
        .filter(|(_, r)| r.state.is_terminal())
        .map(|(k, r)| (k.clone(), r.created_at))
        .collect();
    if terminal.len() <= max {
        return;
    }
    // Oldest first; among equal `created_at`, order by key so eviction is stable.
    terminal.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let drop_count = terminal.len() - max;
    for (key, _) in terminal.into_iter().take(drop_count) {
        requests.remove(&key);
        // Best-effort: a failed remove leaves a stale on-disk record that the
        // next prune retries — it never resurrects an evicted request.
        let _ = store.remove(&key);
    }
}

/// Say that a replay floor could not be read, and that the event it would have
/// bounded is being refused because of it.
///
/// `Option<u64>` has no way to say "unknown": `Ok(None)` means *nothing has
/// ever been stored for this slot*, which legitimately admits any `issuedAt`,
/// while an `Err` means *something may well be stored and we cannot see it*.
/// Flattening the two together handed `verify_clause` "no floor" whenever the
/// store had an EIO, a permissions change, or a `ClauseRec` that would not
/// parse — and no floor is exactly what a replay needs: last month's wider
/// schedule, or a `revoked: true` budget, accepted and written down as
/// current. So an unreadable floor refuses the event instead. A refused clause
/// is re-offered on the next poll (the cursor lookback sees to that), so the
/// cost of being wrong here is a delay; the cost of the other direction is a
/// rollback that sticks.
fn warn_unreadable_floor(slot: &str, e: charter_sys::error::SysError) {
    eprintln!(
        "charter: rollback floor for {slot} is unreadable ({e}) — refusing this event rather \
         than accepting it with no replay protection"
    );
}

/// What one brokered poll actually took in.
///
/// Exists because the Android `PollResult` reported `clausesSeen: 0` and
/// `clausesAccepted: 0` on this path FOREVER — the JNI poll fed
/// `poll_absorb` an empty vec whenever a broker was present, since the broker
/// had already done the real work and returned nothing to say so. A guardian's
/// clause could arrive and apply perfectly while the only diagnostic anyone
/// had insisted nothing had ever been delivered. That cost an hour of chasing
/// a phantom key-rotation bug on the Blue Tablet (2026-07-29); a counter that
/// reads zero when it means "unknown" is worse than no counter at all.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PollCounts {
    /// Clause wraps that unwrapped into a parseable payload this round.
    pub clauses_seen: u32,
    /// …of those, the ones that authenticated and were stored.
    pub clauses_accepted: u32,
    /// A guardian-signed RELEASE authenticated and was applied this round: the
    /// pairing and every stored clause are GONE from disk (S7).
    ///
    /// Returned rather than acted on here because the spine has no business
    /// restarting a daemon. The caller owns what "this device is no longer
    /// managed" means on its platform — on Linux, a `systemctl try-restart`
    /// that brings charterd back through its unpaired arm.
    pub released: bool,
}

/// The privileged broker spine, generic over the system layer + transport.
pub struct Broker<S: SystemLayer, T: TransportFacade, E: Entropy> {
    sys: S,
    transport: T,
    entropy: E,
    enactors: EnactorRegistry,
    events: Box<dyn EventSink>,
    subject: PubKey,
    state: Mutex<State>,
}

impl<S: SystemLayer, T: TransportFacade, E: Entropy> Broker<S, T, E> {
    /// Construct + boot-resubscribe: reload persisted pending records.
    pub fn new(
        sys: S,
        transport: T,
        entropy: E,
        enactors: EnactorRegistry,
        events: Box<dyn EventSink>,
        subject: PubKey,
    ) -> Self {
        let mut requests = BTreeMap::new();
        if let Ok(rows) = sys.pending().list() {
            for (key, json) in rows {
                if let Ok(mut rec) = serde_json::from_str::<RequestRecord>(&json) {
                    // M9: a record left Enacting by a crash mid-enact cannot
                    // resume — its verified grant is gone (consumed, never
                    // persisted) and re-verification is impossible. Reconcile it
                    // to a terminal Failed so it is not stuck forever; the user
                    // re-requests (the enactors are idempotent, so a redo is safe).
                    if rec.state == RequestState::Enacting {
                        rec.state = RequestState::Failed;
                        rec.detail =
                            Some("interrupted before completion; please re-request".into());
                        if let Ok(j) = serde_json::to_string(&rec) {
                            let _ = sys.pending().put(&key, &j);
                        }
                    }
                    requests.insert(key, rec);
                }
            }
        }
        let broker = Broker {
            sys,
            transport,
            entropy,
            enactors,
            events,
            subject,
            state: Mutex::new(State {
                requests,
                cursor: 0,
            }),
        };
        // Bound any terminal history accumulated by earlier runs (this eviction
        // is new; a device upgraded into it may hold an unbounded backlog).
        broker.prune_terminal();
        broker
    }

    /// Access the system layer (tests).
    pub fn sys(&self) -> &S {
        &self.sys
    }

    /// Access the transport (tests).
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn now(&self) -> u64 {
        self.sys.clock().now_utc()
    }

    fn gen_ids(&self) -> (ReqId, Nonce) {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        self.entropy.fill(&mut a);
        self.entropy.fill(&mut b);
        (ReqId::from_bytes(a), Nonce::from_bytes(b))
    }

    fn persist(&self, rec: &RequestRecord) -> Result<(), BrokerError> {
        let json = serde_json::to_string(rec).map_err(|e| BrokerError::Invalid(e.to_string()))?;
        self.sys.pending().put(&rec.req_id.to_hex(), &json)?;
        Ok(())
    }

    /// Enforce the terminal-record cap over memory + disk (see
    /// [`MAX_TERMINAL_RECORDS`]). Called after every transition that can produce a
    /// terminal record, so the persisted store never grows without bound.
    fn prune_terminal(&self) {
        let mut st = self.state.lock().expect("state lock");
        prune_terminal_records(&mut st.requests, self.sys.pending(), MAX_TERMINAL_RECORDS);
    }

    /// Submit a brokered request: build + publish a REQUEST, persist it Pending,
    /// and return the reqId. **Enacts nothing.**
    pub async fn submit(
        &self,
        op: OpType,
        params: serde_json::Value,
        source_path: Option<String>,
    ) -> Result<ReqId, BrokerError> {
        self.submit_as(op, params, source_path, None).await
    }

    /// [`Broker::submit`], recording WHO asked (S8).
    ///
    /// `caller_uid` must be kernel-attested (SO_PEERCRED via the bus), never
    /// taken from the request body — the whole point is that it is not the
    /// caller's to claim. `None` on a single-user platform.
    pub async fn submit_as(
        &self,
        op: OpType,
        params: serde_json::Value,
        source_path: Option<String>,
        caller_uid: Option<u32>,
    ) -> Result<ReqId, BrokerError> {
        if !params.is_object() {
            return Err(BrokerError::Invalid("params must be a JSON object".into()));
        }
        let now = self.now();
        let (req_id, nonce) = self.gen_ids();
        let payload = RequestPayload {
            v: 1,
            op,
            req_id,
            nonce,
            subject: self.subject,
            machine: self.transport.machine_pubkey(),
            ts: now,
            params,
        };
        let rec = RequestRecord {
            req_id,
            nonce,
            op,
            state: RequestState::Pending,
            created_at: now,
            detail: Some("waiting for approval".into()),
            source_path,
            caller_uid,
        };
        // M16: persist durably BEFORE publishing (and before exposing the record
        // in memory) — a lost Pending record silently drops the guardian's later
        // grant (on_grant routes by the persisted reqId). Fail-closed on persist.
        self.persist(&rec)?;
        {
            let mut st = self.state.lock().expect("state lock");
            st.requests.insert(req_id.to_hex(), rec.clone());
        }
        self.transport
            .publish_request(&payload.to_json(), now)
            .await;
        self.events
            .request_updated(&req_id.to_hex(), RequestState::Pending);
        Ok(req_id)
    }

    /// Handle a delivered (unverified) grant: verify (consume-before-enact),
    /// reduce, enact. A rejected grant never enacts.
    pub async fn on_grant(&self, received: ReceivedGrant) {
        // Route by the (unverified) reqId to find the pending record.
        let payload = match GrantPayload::from_json(&received.grant.content) {
            Ok(p) => p,
            Err(_) => return,
        };
        let key = payload.req_id.to_hex();
        let mut rec = {
            let st = self.state.lock().expect("state lock");
            match st.requests.get(&key) {
                Some(r) if r.state == RequestState::Pending => r.clone(),
                _ => return, // unknown or already resolved
            }
        };

        let now = self.now();
        let pinned = self.transport.pinned_guardian();
        let vp = VerifyParams {
            pinned_guardian: &pinned,
            expected_req_id: &rec.req_id,
            expected_nonce: &rec.nonce,
            // Bind the grant to THIS pending record's op: the record is looked up
            // by the untrusted payload.req_id, so without this the enacted op
            // (grant.op()) could diverge from the op the request's bindings +
            // audit are tagged with. verify_grant rejects a mismatch (OpMismatch).
            expected_op: rec.op,
            now,
        };
        let ev = match verify_grant(&received.grant, &vp, self.sys.consumed_ids()) {
            Ok(vg) => Event::GrantVerified(vg),
            Err(e) => Event::GrantRejected(e),
        };

        let (state1, effects1) = transition(&rec, ev);
        rec.state = state1;
        let mut to_enact = None;
        for eff in effects1 {
            match eff {
                Effect::Enact(vg) => to_enact = Some(vg),
                Effect::Audit(a) => self.transport.emit_audit(a.tags(), now).await,
                Effect::Notify => self.events.request_updated(&key, rec.state),
                Effect::Persist => {
                    // Best-effort in the receive pipeline; the durable guarantee
                    // is the submit-time persist (M16). A failure here is logged
                    // by the port, not fatal to processing the grant.
                    let _ = self.persist(&rec);
                }
            }
        }

        if let Some(vg) = to_enact {
            // verify_grant binds the grant's op to `rec.op` (VerifyError::
            // OpMismatch), so a verified grant can only carry the pending
            // record's op. Assert that invariant before enacting: the enactor
            // dispatches on `vg.op()`, the bindings (source_path) come from
            // `rec`, and the audit is tagged `rec.op` — all three must name the
            // same op, and the op binding guarantees they do.
            debug_assert_eq!(
                vg.op(),
                rec.op,
                "verified grant op must match the pending record's op"
            );
            let ctx = EnactContext {
                source_path: rec.source_path.clone(),
                now_unix: Some(now as i64),
                // M11: time.extend's today-only cap derives from the CLAUSE
                // enforcement tz, not the daemon's ambient runtime tz.
                eod_unix: (vg.op() == OpType::TimeExtend)
                    .then(|| crate::enforcer_runtime::time_extend_eod(&self.sys, now as i64)),
            };
            // M8: a transient enact failure may retry the SAME in-memory verified
            // grant (never re-verify, never re-consume). A terminal failure or an
            // exhausted attempt budget falls through to a fail-safe Failed.
            const MAX_ENACT_ATTEMPTS: u32 = 3;
            let mut attempt = 0;
            let ev2 = loop {
                attempt += 1;
                match self.enactors.enact(&vg, &ctx).await {
                    Ok(_) => break Event::EnactSucceeded,
                    Err(e) if e.is_terminal() || attempt >= MAX_ENACT_ATTEMPTS => {
                        break Event::EnactFailed(e.to_string());
                    }
                    Err(_) => continue,
                }
            };
            let (state2, effects2) = transition(&rec, ev2);
            rec.state = state2;
            for eff in effects2 {
                match eff {
                    Effect::Enact(_) => {}
                    Effect::Audit(a) => self.transport.emit_audit(a.tags(), now).await,
                    Effect::Notify => self.events.request_updated(&key, rec.state),
                    Effect::Persist => {
                        // Best-effort in the receive pipeline; the durable guarantee
                        // is the submit-time persist (M16). A failure here is logged
                        // by the port, not fatal to processing the grant.
                        let _ = self.persist(&rec);
                    }
                }
            }
        }

        {
            let mut st = self.state.lock().expect("state lock");
            st.requests.insert(key, rec);
        }
        // A grant resolves the request to a terminal state (Enacted / Denied /
        // Failed); bound the retained terminal history.
        self.prune_terminal();
    }

    /// Handle a delivered (unverified) clause: authenticate + rollback-protect,
    /// then cache only if valid. The authority check (pinned guardian + monotonic
    /// `issuedAt`) is identical for every clause — `subject` only routes; fail-
    /// closed per child.
    ///
    /// **Routing.** Screen-time clauses (`schedule`/`budget`) are cached under the
    /// per-(subject, kind) `ChildClauseStore`, which the live `MultiChildEnforcer`
    /// is the only reader of. An explicit `subject` names the child; an **absent**
    /// subject means the pairing's **sole subject** (the contract's single-child
    /// back-compat default) — it MUST land in the per-child store too, or the
    /// guardian's remote schedule/budget would be authenticated and cached yet
    /// never freeze/lock anyone (a silent fail-open). `content` stays on the
    /// single-child `ClauseStore`: web filtering is machine-wide and the web-
    /// content enforcer reads that store directly.
    /// Returns whether the clause was authenticated and stored — the input to
    /// the `clausesAccepted` diagnostic. A `false` means seen-but-refused
    /// (unparseable, wrong guardian, or below the replay floor), which is a
    /// very different fault from nothing arriving at all.
    pub async fn on_clause(&self, received: ReceivedClause) -> bool {
        let payload = match ClausePayload::from_json(&received.clause.content) {
            Ok(p) => p,
            Err(_) => return false,
        };
        let store_key = payload.kind.store_key();
        let pinned = self.transport.pinned_guardian();
        let now = self.now();

        // Which child's per-(subject, kind) slot this clause targets, if any. An
        // absent subject on a screen-time clause routes to the pairing's sole
        // subject; `content` has no per-child routing (machine-wide).
        let per_child_subject = match payload.subject {
            Some(subject) => Some(subject),
            None => match payload.kind {
                // Per-child dimensions route to the sole ward when subject absent.
                ClauseKind::Schedule
                | ClauseKind::Budget
                | ClauseKind::Apps
                | ClauseKind::Learning
                | ClauseKind::AppRules
                | ClauseKind::Tethering
                | ClauseKind::Update
                | ClauseKind::Lifeline
                | ClauseKind::Buckets
                | ClauseKind::Maintenance
                | ClauseKind::Gift
                | ClauseKind::StandDown
                | ClauseKind::Listening
                | ClauseKind::AlwaysAvailable => Some(self.subject),
                // Content is machine-wide (no per-child routing).
                ClauseKind::Content => None,
            },
        };

        match per_child_subject {
            Some(subject) => {
                // Per-child: rollback bound + cache are scoped to (subject, kind),
                // so a hostile relay cannot revert one child's clause or replay
                // another child's into theirs.
                let subject_hex = subject.to_hex();
                let prev = match self
                    .sys
                    .child_clauses()
                    .highest_issued_at(&subject_hex, store_key)
                {
                    Ok(prev) => prev,
                    Err(e) => {
                        warn_unreadable_floor(
                            &format!("child clause {subject_hex}/{store_key}"),
                            e,
                        );
                        return false;
                    }
                };
                if let Ok(vc) = verify_clause(&received.clause, &pinned, payload.kind, prev, now) {
                    let body = serde_json::to_string(vc.body()).unwrap_or_default();
                    let _ = self.sys.child_clauses().put_child_clause(
                        &subject_hex,
                        store_key,
                        vc.issued_at(),
                        &body,
                    );
                    return true;
                }
                false
            }
            None => {
                let prev = match self.sys.clauses().highest_issued_at(store_key) {
                    Ok(prev) => prev,
                    Err(e) => {
                        warn_unreadable_floor(&format!("clause {store_key}"), e);
                        return false;
                    }
                };
                if let Ok(vc) = verify_clause(&received.clause, &pinned, payload.kind, prev, now) {
                    let body = serde_json::to_string(vc.body()).unwrap_or_default();
                    let _ = self
                        .sys
                        .clauses()
                        .put_clause(store_key, vc.issued_at(), &body);
                    return true;
                }
                false
            }
        }
    }

    /// Handle a delivered (unverified) USAGE_SYNC: authenticate against the
    /// pinned guardian + per-subject `ts` replay floor, then cache the latest
    /// consolidated view in the per-(subject, key) store under
    /// [`USAGE_SYNC_STORE_KEY`]. The store's monotonic floor IS the replay
    /// protection, and `clear_for` on release wipes the view with the clauses.
    /// Staleness only under-counts the pool, so rejecting is always safe.
    pub async fn on_usage_sync(&self, event: charter_primitives::NostrEvent) {
        let Ok(payload) = UsageSyncPayload::from_json(&event.content) else {
            return;
        };
        let subject_hex = payload.subject.to_hex();
        let prev = match self
            .sys
            .child_clauses()
            .highest_issued_at(&subject_hex, USAGE_SYNC_STORE_KEY)
        {
            Ok(prev) => prev,
            Err(e) => {
                warn_unreadable_floor(&format!("usage sync {subject_hex}"), e);
                return;
            }
        };
        let pinned = self.transport.pinned_guardian();
        let now = self.now();
        if let Ok(vs) = verify_usage_sync(&event, &pinned, prev, now) {
            let _ = self.sys.child_clauses().put_child_clause(
                &subject_hex,
                USAGE_SYNC_STORE_KEY,
                vs.ts(),
                &vs.payload().to_json(),
            );
        }
    }

    /// The `paired_at` of the pinned pairing — the monotonic floor a RELEASE
    /// must clear. `0` when unknown (a legacy pairing shape, or unreadable),
    /// which admits every release: the same posture the Android warden takes,
    /// and the safe one, since a release only ever LOOSENS.
    fn paired_at(&self) -> u64 {
        self.sys
            .pairing()
            .load()
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| v.get("paired_at").and_then(|x| x.as_u64()))
            .unwrap_or(0)
    }

    /// Authenticate and apply a guardian-signed RELEASE — the remote unpair
    /// (S7, review 2026-08-07).
    ///
    /// `poll_releases` and `verify_release` have existed since the Android
    /// warden was written; nothing on Linux ever called them, so a guardian
    /// pressing "Disconnect" in MyCharter published a perfectly good release
    /// into a void and their laptop stayed managed forever, with the app
    /// showing it as gone. The direction was safe — a ward gained nothing —
    /// but a management function that silently does nothing is its own kind
    /// of broken.
    ///
    /// Mirrors `Warden::try_apply_releases` on Android, floor included:
    ///
    /// - `verify_release` binds the pinned guardian AND this machine's pubkey,
    ///   and enforces the 48h freshness window;
    /// - `issued_at >= paired_at` rejects a release that predates the CURRENT
    ///   pairing epoch. Without it, a re-paired device gets un-paired by its
    ///   own earlier release, redelivered inside the lookback window — which
    ///   is not theoretical, it happened live on 2026-07-22 (issue #49
    ///   sibling).
    ///
    /// Returns true when a release was applied. Everything on disk that made
    /// this a managed device is gone by then; the caller restarts.
    async fn on_release(&self, event: &charter_primitives::NostrEvent, now: u64) -> bool {
        let guardian = self.transport.pinned_guardian();
        let machine = self.transport.machine_pubkey();
        if charter_verify::verify_release(event, &guardian, &machine, now).is_err() {
            return false;
        }
        let Ok(payload) = charter_proto::ReleasePayload::from_json(&event.content) else {
            return false;
        };
        let paired_at = self.paired_at();
        if payload.issued_at < paired_at {
            return false;
        }
        // Order matters: the clauses first, so a crash between the two leaves a
        // still-pinned device with no rules rather than an unpinned device
        // still enforcing rules nobody can lift.
        let _ = self.sys.child_clauses().clear_for(&self.subject.to_hex());
        let _ = self.sys.pairing().clear();
        // Say it on the audit feed BEFORE the caller tears the daemon down —
        // this is the last thing this process will ever have a guardian to
        // tell, and "your laptop stopped being managed" is worth a line.
        self.transport
            .emit_audit(
                vec![
                    vec!["event".into(), "release-applied".into()],
                    vec!["issued_at".into(), payload.issued_at.to_string()],
                ],
                now,
            )
            .await;
        true
    }

    /// Poll the transport once and process all delivered grants + clauses.
    pub async fn poll_once(&self) -> PollCounts {
        let now = self.now();
        let cursor = {
            let st = self.state.lock().expect("state lock");
            st.cursor
        };
        let mut counts = PollCounts::default();
        // RELEASE first, and RETURN on it (S7). A release means this device is
        // no longer managed by anyone; ingesting the same round's clauses
        // afterwards would write rules straight back into the store we just
        // emptied, and the caller is about to tear the daemon down regardless.
        for (event, _seal_author) in self.transport.poll_releases(cursor, now).await {
            if self.on_release(&event, now).await {
                counts.released = true;
                return counts;
            }
        }
        for clause in self.transport.poll_clauses(cursor, now).await {
            counts.clauses_seen += 1;
            if self.on_clause(clause).await {
                counts.clauses_accepted += 1;
            }
        }
        for (event, _seal_author) in self.transport.poll_usage_syncs(cursor, now).await {
            self.on_usage_sync(event).await;
        }
        crate::curator_sync::refresh_curator_lists(&self.sys, &self.transport).await;
        for grant in self.transport.poll_grants(cursor, now).await {
            self.on_grant(grant).await;
        }
        let mut st = self.state.lock().expect("state lock");
        // Lag the cursor a jitter window behind now (see `POLL_LOOKBACK_SECS`) so
        // a slightly-backdated or late-redelivered guardian event is never
        // permanently filtered out by the relay's `since` bound.
        st.cursor = now.saturating_sub(POLL_LOOKBACK_SECS);
        counts
    }

    /// The current subscription cursor (the `since` used by the next poll).
    /// Introspection for tests — see [`POLL_LOOKBACK_SECS`].
    pub fn cursor(&self) -> u64 {
        self.state.lock().expect("state lock").cursor
    }

    /// Status of one request (empty key = all), newest first.
    pub fn status(&self, req_id: &str) -> Vec<RequestRecord> {
        let st = self.state.lock().expect("state lock");
        if req_id.is_empty() {
            let mut v: Vec<_> = st.requests.values().cloned().collect();
            v.sort_by_key(|r| std::cmp::Reverse(r.created_at));
            v
        } else {
            st.requests.get(req_id).cloned().into_iter().collect()
        }
    }

    /// [`Broker::status`], scoped to what `uid` is allowed to see (S8).
    ///
    /// A shared family laptop has several children on it, and the unscoped
    /// read handed any local account every other account's pending asks —
    /// what they asked for, and when. That is one sibling's business, not the
    /// other's, and the req_ids it leaked were the input to `cancel`.
    pub fn status_for(&self, uid: u32, req_id: &str) -> Vec<RequestRecord> {
        self.status(req_id)
            .into_iter()
            .filter(|r| r.visible_to(uid))
            .collect()
    }

    /// Newest-first list, bounded by `limit` (0 = unbounded).
    pub fn list(&self, limit: usize) -> Vec<RequestRecord> {
        let mut v = self.status("");
        if limit > 0 {
            v.truncate(limit);
        }
        v
    }

    /// [`Broker::list`], scoped to `uid` (S8). The limit applies AFTER the
    /// scoping, so a caller asking for ten of their own gets ten of their own
    /// rather than whatever survives filtering someone else's ten.
    pub fn list_for(&self, uid: u32, limit: usize) -> Vec<RequestRecord> {
        let mut v = self.status_for(uid, "");
        if limit > 0 {
            v.truncate(limit);
        }
        v
    }

    /// [`Broker::cancel`], refusing to withdraw someone else's ask (S8).
    ///
    /// Impact was bounded — a cancel only withdraws a PENDING request, and a
    /// grant still has to be guardian-signed — but "your sibling can make
    /// your request disappear before your parent ever sees it" is its own
    /// small cruelty, and req_ids were enumerable through `ListRequests`.
    pub fn cancel_as(&self, uid: u32, req_id: &str) -> bool {
        {
            let st = self.state.lock().expect("state lock");
            match st.requests.get(req_id) {
                Some(rec) if rec.visible_to(uid) => {}
                // Same answer for "not yours" and "does not exist": a distinct
                // refusal would confirm the reqId is real.
                _ => return false,
            }
        }
        self.cancel(req_id)
    }

    /// Withdraw a pending request. Returns true if it was pending.
    pub fn cancel(&self, req_id: &str) -> bool {
        let mut st = self.state.lock().expect("state lock");
        if let Some(rec) = st.requests.get_mut(req_id) {
            if rec.state == RequestState::Pending {
                rec.state = RequestState::Cancelled;
                let snapshot = rec.clone();
                drop(st);
                let _ = self.persist(&snapshot);
                // A cancel is terminal; bound the retained terminal history.
                self.prune_terminal();
                self.events.request_updated(req_id, RequestState::Cancelled);
                return true;
            }
        }
        false
    }
}

#[cfg(all(test, feature = "mock"))]
mod prune_tests {
    use super::*;
    use charter_primitives::{Nonce, ReqId};
    use charter_proto::OpType;
    use charter_sys::persistence::{MockDisk, MockPendingStore};

    fn rec(state: RequestState, created_at: u64) -> RequestRecord {
        RequestRecord {
            req_id: ReqId::from_bytes([0; 32]),
            nonce: Nonce::from_bytes([0; 32]),
            op: OpType::InstallFlatpak,
            state,
            created_at,
            detail: None,
            source_path: None,
            caller_uid: None,
        }
    }

    /// Seed a map + store with the same records under string keys, so we can
    /// assert eviction hit BOTH memory and the persisted store.
    fn seed(
        records: &[(&str, RequestState, u64)],
    ) -> (BTreeMap<String, RequestRecord>, MockPendingStore) {
        let store = MockPendingStore::new(MockDisk::new());
        let mut map = BTreeMap::new();
        for (key, state, created_at) in records {
            let r = rec(*state, *created_at);
            store.put(key, &serde_json::to_string(&r).unwrap()).unwrap();
            map.insert((*key).to_string(), r);
        }
        (map, store)
    }

    #[test]
    fn terminal_records_are_bounded_and_pending_are_retained() {
        // Five terminal records (created_at 10..50) plus two Pending — one of
        // which (p05) is the OLDEST record of all, to prove pending are exempt
        // from the age-based eviction entirely.
        let (mut map, store) = seed(&[
            ("p05", RequestState::Pending, 5),
            ("t10", RequestState::Enacted, 10),
            ("t20", RequestState::Denied, 20),
            ("t30", RequestState::Failed, 30),
            ("t40", RequestState::Cancelled, 40),
            ("t50", RequestState::Expired, 50),
            ("p60", RequestState::Pending, 60),
        ]);

        // Keep the 3 most-recent terminal records; the 2 oldest terminal go.
        prune_terminal_records(&mut map, &store, 3);

        // Memory: the two oldest terminal evicted, the newer three kept, and
        // BOTH pending retained (including the globally-oldest p05).
        assert!(!map.contains_key("t10"));
        assert!(!map.contains_key("t20"));
        for k in ["t30", "t40", "t50", "p05", "p60"] {
            assert!(map.contains_key(k), "{k} must be retained");
        }

        // Disk: the same eviction reached the persisted store.
        let on_disk: Vec<String> = store.list().unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(on_disk.len(), 5);
        assert!(!on_disk.contains(&"t10".to_string()));
        assert!(!on_disk.contains(&"t20".to_string()));
        assert!(on_disk.contains(&"p05".to_string()));
        assert!(on_disk.contains(&"p60".to_string()));
    }

    #[test]
    fn pending_records_are_never_evicted_even_past_the_cap() {
        // Ten Pending records, cap of 3: none may be evicted (only terminal
        // records are counted against the bound).
        let recs: Vec<(String, RequestState, u64)> = (0..10)
            .map(|i| (format!("p{i:02}"), RequestState::Pending, i as u64))
            .collect();
        let borrowed: Vec<(&str, RequestState, u64)> =
            recs.iter().map(|(k, s, c)| (k.as_str(), *s, *c)).collect();
        let (mut map, store) = seed(&borrowed);

        prune_terminal_records(&mut map, &store, 3);

        assert_eq!(map.len(), 10, "no Pending record may be evicted");
        assert_eq!(store.list().unwrap().len(), 10);
    }

    #[test]
    fn below_cap_is_a_noop() {
        let (mut map, store) = seed(&[
            ("t10", RequestState::Enacted, 10),
            ("t20", RequestState::Denied, 20),
        ]);
        prune_terminal_records(&mut map, &store, MAX_TERMINAL_RECORDS);
        assert_eq!(map.len(), 2);
        assert_eq!(store.list().unwrap().len(), 2);
    }
}
