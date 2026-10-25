//! The broker: submit -> publish, and the receive pipeline (verify -> reduce ->
//! enact) for grants, plus clause authentication. Single-use is race-free via
//! the atomic `check_and_consume` (consume happens *inside* `verify_grant`,
//! before any enact). In production a single task owns the broker; here it is
//! internally synchronized so the headless tests drive it directly.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use charter_link::AckResult;
use charter_primitives::{EventId, Nonce, NostrEvent, PubKey, ReqId};
use charter_proto::{
    ClauseKind, ClausePayload, GrantPayload, OpType, RequestPayload, UsageSyncPayload,
    USAGE_SYNC_STORE_KEY,
};
use charter_sys::error::SysResult;
use charter_sys::persistence::{
    ChildClauseStore, ClauseStore, PairingStore, PendingStore, ReplayFloor,
};
use charter_sys::{Clock, SystemLayer};
use charter_transport::{Entropy, ReceivedClause, ReceivedGrant};
use charter_verify::{
    verify_clause, verify_grant, verify_usage_sync, ClauseError, UsageSyncError, VerifyError,
    VerifyParams,
};

use crate::enactor::{EnactContext, EnactorRegistry};
use crate::error::BrokerError;
use crate::lifecycle::{transition, Effect, Event, RequestRecord, RequestState};
use crate::ports::EventSink;
use crate::transport_facade::{LocalInbound, TransportFacade};

struct State {
    requests: BTreeMap<String, RequestRecord>,
    cursor: u64,
    /// Submit timestamps per caller, newest last, pruned to
    /// [`SUBMIT_WINDOW_SECS`] on every check — the sliding window behind
    /// [`MAX_SUBMITS_PER_HOUR`]. In memory only, deliberately: a restart is a
    /// far heavier thing than the cap it would reset, the outstanding cap
    /// (which IS persisted, being the record map) still holds across it, and
    /// persisting a rate-limiter's ticks would write to disk on every ask.
    /// Keyed by `caller_uid`, so one child's flood cannot spend a sibling's
    /// budget; `None` (root / unidentified) is its own key.
    submits: BTreeMap<Option<u32>, VecDeque<u64>>,
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
/// evicted BY THE CAP** — they carry in-flight guardian decisions and the
/// boot-resubscribe set, so dropping one would silently strand a request. They
/// leave the non-terminal set only by being *answered*, or — for Pending — by
/// ageing out through [`PENDING_TTL_SECS`], which turns them terminal first
/// (`Event::Expire`) so this cap then bounds them like any other history. This
/// is a generous history window for a family device; the number is a durability
/// bound, not a UX limit.
const MAX_TERMINAL_RECORDS: usize = 256;

/// How long a Pending request waits for its guardian before it ages out
/// (`Event::Expire` -> `Expired`, which is terminal and therefore subject to
/// [`MAX_TERMINAL_RECORDS`]).
///
/// Before this, `Event::Expire` existed in the reducer and was emitted
/// **nowhere**: a Pending record was immortal, so a ward looping
/// `charter ask-for-more` grew both the in-memory map and the on-disk
/// `PendingStore` without bound, and every one of those records stayed eligible
/// to be matched by a grant forever. A day is the right window for a
/// human-answered ask: long enough that "I asked last night, Dad looked at it
/// over breakfast" still works, short enough that yesterday's ask cannot be
/// answered into effect a fortnight later.
///
/// **Only Pending expires.** `Enacting` is mid-enact with a consumed grant
/// behind it; timing it out here would race the enactor. A crash leaves it to
/// [`Broker::new`]'s M9 reconcile instead.
const PENDING_TTL_SECS: u64 = 24 * 3600;

/// How many requests one caller may have *outstanding* (Pending or Enacting) at
/// once. Over the cap, a submit is refused before any id is generated or
/// anything is published.
///
/// The number is a "how many asks can a person be genuinely waiting on"
/// judgement, not a resource bound — the resource bound is that this, with
/// [`MAX_SUBMITS_PER_HOUR`], is what keeps a looping ward from filling their
/// guardian's Approvals screen (and the event sink, and the relay inbox) with
/// the same ask ten thousand times. A guardian answering one frees a slot
/// immediately.
const MAX_PENDING_PER_CALLER: usize = 8;

/// How many submits one caller may make inside [`SUBMIT_WINDOW_SECS`],
/// regardless of how quickly they are answered. The outstanding cap alone would
/// not bind a ward whose asks are auto-denied (or cancelled) as fast as they are
/// made; this one does.
const MAX_SUBMITS_PER_HOUR: usize = 12;

/// The sliding window [`MAX_SUBMITS_PER_HOUR`] is counted over.
const SUBMIT_WINDOW_SECS: u64 = 3600;

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

/// What one expired record still needs doing to it, outside the state lock:
/// the key, the record as the reducer left it, and whether the reducer asked
/// for a `Notify` / a `Persist`.
type ExpiredRecord = (String, RequestRecord, bool, bool);

/// Age out every `Pending` record whose `created_at` + [`PENDING_TTL_SECS`] has
/// passed, by running it through the reducer's `Event::Expire`. Returns one
/// entry per record transitioned, for the caller to carry the effects out on.
///
/// **Only `Pending` ages out.** `Enacting` has a consumed grant behind it and an
/// enactor in flight; expiring it here would race that enactor and leave a
/// half-applied change described as "never answered". Terminal states are inert
/// by definition.
///
/// Pure over its inputs — no `Broker`, no clock, no IO — so the TTL rule is
/// unit-testable directly, the same way [`prune_terminal_records`] is.
fn expire_stale_pending_records(
    requests: &mut BTreeMap<String, RequestRecord>,
    now: u64,
) -> Vec<ExpiredRecord> {
    let mut expired = Vec::new();
    for (key, rec) in requests.iter_mut() {
        if rec.state != RequestState::Pending {
            continue;
        }
        // `saturating_add` so a record with an absurd `created_at` (a clock that
        // jumped, a hand-edited store) cannot wrap into the past and expire the
        // instant it is written.
        if rec.created_at.saturating_add(PENDING_TTL_SECS) > now {
            continue;
        }
        let (next, effects) = transition(rec, Event::Expire);
        rec.state = next;
        let (mut notify, mut persist) = (false, false);
        for eff in effects {
            match eff {
                Effect::Notify => notify = true,
                Effect::Persist => persist = true,
                // An expiry enacts nothing and audits nothing; a future reducer
                // that wanted either would need the receive pipeline's
                // machinery, not this sweep.
                Effect::Enact(_) | Effect::Audit(_) => {}
            }
        }
        expired.push((key.clone(), rec.clone(), notify, persist));
    }
    expired
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

/// What a clause whose replay floor cannot be read comes to. It is never
/// stored — with no floor there is no replay protection — but if it
/// authenticates as the pinned guardian's it is counted as `WriteFailed`, so
/// `clausesWriteFailed` tells the guardian their clause did not land (M3: a
/// torn record with no floor sidecar used to refuse every correction
/// silently, forever). A clause that does not authenticate stays `Refused`, so
/// nobody else can move the counter.
fn unreadable_floor_outcome(
    received: &ReceivedClause,
    pinned: &PubKey,
    kind: ClauseKind,
    now: u64,
) -> ClauseDisposition {
    match verify_clause(&received.clause, pinned, kind, None, now) {
        Ok(_) => ClauseDisposition::WriteFailed,
        Err(_) => ClauseDisposition::Refused,
    }
}

/// Say that an authenticated clause's store write itself failed — distinct
/// from `warn_unreadable_floor`, which is about the *read* that gates
/// acceptance. This is logged at error level because it is exactly the
/// failure [`PollCounts`]'s doc comment describes: the guardian is told the
/// clause landed while the device keeps enforcing whatever was there before.
fn warn_write_failed(slot: &str, e: charter_sys::error::SysError) {
    eprintln!(
        "charter: clause for {slot} authenticated but the store write failed ({e}) — the \
         device will keep enforcing the previous clause until this is retried"
    );
}

/// What became of a delivered clause after authentication was attempted.
///
/// Deliberately distinct from a `bool`: folding `Refused` (unparseable,
/// wrong guardian, or below the replay floor — see `on_clause`'s doc
/// comment) and `WriteFailed` (authenticated, but the store write itself
/// returned `Ok(false)`'s sibling error — a full disk, a read-only
/// `/var/lib/charter`, an EIO) into the same "not accepted" bit is exactly
/// what let a write failure report as "delivered and applied" to the
/// guardian while the device kept enforcing the previous clause. See B10,
/// the 2026-09-21 review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseOutcome {
    /// Authenticated and durably stored.
    Stored,
    /// Not authenticated (unparseable payload, wrong/unpinned guardian, or
    /// `issuedAt` at/below the replay floor — the store's `put_*` returned
    /// `Ok(false)`), or the replay floor itself could not be read.
    Refused,
    /// Authenticated — this WAS the guardian's clause, past the replay floor
    /// — but the store write failed. Must never be reported as accepted.
    WriteFailed,
}

/// What became of a delivered clause, in the detail a signed local-link ACK
/// needs. [`ClauseOutcome`] is its projection for every existing caller:
/// `Unchanged` and `Superseded` were, and still are, `Refused` there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseDisposition {
    /// Authenticated and durably stored as the slot's clause.
    Stored,
    /// Authenticated, and the slot already holds this clause: the stored
    /// record is at this `issuedAt` (a record floor, not a sidecar) with the
    /// same body.
    Unchanged,
    /// Authenticated, but at or below the slot's floor and not the clause it
    /// holds: the newer (or tied, different) clause stays in effect.
    Superseded,
    /// Not authenticated (unparseable, wrong kind or shape, bad id or
    /// signature, not the pinned guardian), or its floor could not be read
    /// and it does not authenticate either.
    Refused,
    /// Authenticated, but the store write (or the floor read gating it)
    /// failed.
    WriteFailed,
}

impl ClauseDisposition {
    fn outcome(self) -> ClauseOutcome {
        match self {
            ClauseDisposition::Stored => ClauseOutcome::Stored,
            ClauseDisposition::WriteFailed => ClauseOutcome::WriteFailed,
            ClauseDisposition::Unchanged
            | ClauseDisposition::Superseded
            | ClauseDisposition::Refused => ClauseOutcome::Refused,
        }
    }

    fn ack(self) -> AckResult {
        match self {
            ClauseDisposition::Stored => AckResult::Applied,
            ClauseDisposition::Unchanged => AckResult::Unchanged,
            ClauseDisposition::Superseded => AckResult::Superseded,
            ClauseDisposition::Refused => AckResult::Refused,
            ClauseDisposition::WriteFailed => AckResult::Failed,
        }
    }
}

/// Which store a clause lives in: a child's per-(subject, kind) slot, or the
/// machine-wide one (`content`).
enum ClauseSlot {
    Child(String),
    Machine,
}

impl ClauseSlot {
    /// The slot's name in log lines (unchanged from before the split).
    fn describe(&self, store_key: u16) -> String {
        match self {
            ClauseSlot::Child(subject_hex) => format!("child clause {subject_hex}/{store_key}"),
            ClauseSlot::Machine => format!("clause {store_key}"),
        }
    }
}

/// Why a grant routed to a Pending record did not verify, reduced to what an
/// ACK can say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantRejection {
    /// Authentic and bound to the request, but past its `exp`.
    Expired,
    /// Authentic and bound, but its reqId was already consumed while the
    /// request is still Pending: an earlier copy was verified but the
    /// request's progress was never recorded (a failed persist and a crash).
    /// The device cannot show it enacted, so this is reported as `Failed`,
    /// never as an answer already held.
    Replayed,
    /// The consumed-id store failed: authentic, but not recordable now.
    StoreFailed,
    /// Anything else: forged, wrong guardian, malformed, binding mismatch,
    /// not yet valid.
    Invalid,
}

impl GrantRejection {
    fn of(e: &VerifyError) -> Self {
        match e {
            VerifyError::Expired => GrantRejection::Expired,
            VerifyError::Replayed => GrantRejection::Replayed,
            VerifyError::Store(_) => GrantRejection::StoreFailed,
            _ => GrantRejection::Invalid,
        }
    }
}

/// What became of a delivered grant. `on_grant` returns nothing, as it always
/// has; this is for the local link's ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantDisposition {
    /// The payload did not parse, so it routes to no request.
    Malformed,
    /// No request record for its reqId.
    NoRecord,
    /// The request was no longer Pending when this copy arrived.
    AlreadyResolved(RequestState),
    /// Routed to a Pending request, and verification refused it.
    Rejected(GrantRejection),
    /// Routed, verified, and the request moved to this state.
    Resolved(RequestState),
}

impl GrantDisposition {
    fn ack(self) -> AckResult {
        use RequestState as S;
        match self {
            GrantDisposition::Malformed | GrantDisposition::NoRecord => AckResult::Refused,
            GrantDisposition::AlreadyResolved(state) => match state {
                // Answered by an earlier copy (Enacting is only visible to a
                // caller outside the handler turn).
                S::Enacted | S::Denied | S::Enacting => AckResult::Unchanged,
                // The ask aged out or was withdrawn before this copy came.
                S::Expired | S::Cancelled => AckResult::Expired,
                S::Failed => AckResult::Failed,
                S::Rejected | S::Pending => AckResult::Refused,
            },
            GrantDisposition::Rejected(r) => match r {
                GrantRejection::Expired => AckResult::Expired,
                GrantRejection::Replayed => AckResult::Failed,
                GrantRejection::StoreFailed => AckResult::Failed,
                GrantRejection::Invalid => AckResult::Refused,
            },
            GrantDisposition::Resolved(state) => match state {
                S::Enacted | S::Denied => AckResult::Applied,
                S::Failed => AckResult::Failed,
                S::Expired | S::Cancelled => AckResult::Expired,
                S::Pending | S::Enacting | S::Rejected => AckResult::Refused,
            },
        }
    }
}

/// How [`Broker::ingest_local`] treats what it cannot finish by itself.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LocalIngestOptions {
    /// `true`: a RELEASE is verified and applied here, exactly as on the
    /// poll path (Linux). `false`: it is handed back untouched in
    /// [`LocalIngestOutcome::release`] for the platform's own release path
    /// (Android's `try_apply_releases`), and reported `Ignored`.
    pub handle_release: bool,
}

/// What [`Broker::ingest_local`] did with one wrap — the content of the
/// ward's signed ACK, plus what the caller needs to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIngestOutcome {
    pub result: AckResult,
    /// The inner event's kind, when the wrap opened.
    pub inner_kind: Option<u16>,
    /// The inner event's id, when the wrap opened into a signed event.
    pub inner_id: Option<EventId>,
    /// The (unverified) reqId a GRANT names, when it parsed.
    pub req_id: Option<ReqId>,
    /// A RELEASE authenticated and was applied: the pairing and every stored
    /// clause are gone, and the caller restarts, as for
    /// [`PollCounts::released`].
    pub released: bool,
    /// With `handle_release: false`, the unwrapped RELEASE `(event,
    /// seal_author)`, unverified, for the platform's release path.
    pub release: Option<(NostrEvent, PubKey)>,
}

impl LocalIngestOutcome {
    fn refused() -> Self {
        LocalIngestOutcome {
            result: AckResult::Refused,
            inner_kind: None,
            inner_id: None,
            req_id: None,
            released: false,
            release: None,
        }
    }
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
    /// …of the ones that authenticated, the ones whose store write itself
    /// failed (a full disk, a read-only `/var/lib/charter`, an EIO) — see
    /// [`ClauseOutcome::WriteFailed`]. NOT folded into `clauses_accepted`:
    /// a write failure means the device keeps enforcing the previous clause
    /// while the guardian believes the new one is live, which is the exact
    /// silent-fail-open [`ClauseOutcome`]'s doc comment exists to end. Logged
    /// at error level as it happens (see `warn_write_failed`) as well as
    /// counted here, so it survives even if nobody is polling `PollCounts`.
    pub clauses_write_failed: u32,
    /// A guardian-signed RELEASE authenticated and was applied this round: the
    /// pairing and every stored clause are GONE from disk (S7).
    ///
    /// Returned rather than acted on here because the spine has no business
    /// restarting a daemon. The caller owns what "this device is no longer
    /// managed" means on its platform — on Linux, a `systemctl try-restart`
    /// that brings charterd back through its unpaired arm.
    pub released: bool,
    /// At least one poll this round could not reach ANY relay in the set
    /// (`charter_sys::relay::RelayIoError::Unreachable`, surfaced through
    /// [`crate::transport_facade::PollHealth`]). Distinct from every count
    /// above being zero, which is equally what "reached, nothing new" looks
    /// like — a device whose relay set has gone bad must not read as a
    /// device with a perfectly quiet guardian.
    pub relays_unreachable: bool,
    /// Per-relay publish failures this round (each failed relay on a single
    /// publish counts once).
    pub publish_failed: u32,
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
    /// One handler at a time: every `on_*` call from [`Broker::poll_once`]
    /// and [`Broker::ingest_local`] holds this turn, so a wrap arriving over
    /// the local link and its relay copy can never interleave inside the
    /// same handler. Taken around each handler call only — never across a
    /// relay query.
    handlers: tokio::sync::Mutex<()>,
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
                submits: BTreeMap::new(),
            }),
            handlers: tokio::sync::Mutex::new(()),
        };
        // A daemon that was off for a week comes up with a week-old Pending set.
        // Age it out HERE, before anything can poll: otherwise the first poll
        // after boot is free to match a grant against an ask from last Tuesday,
        // and the outstanding cap is spent on requests nobody is still waiting
        // for. (Expiring also turns them terminal, so the prune below bounds
        // them.)
        broker.expire_stale_pending();
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

    /// Age out every Pending record past [`PENDING_TTL_SECS`] via the reducer's
    /// `Event::Expire`, then bound the terminal history it just added to.
    ///
    /// Run at construction and at the **top** of every [`Broker::poll_once`] —
    /// before the transport is asked for anything. The ordering is the point:
    /// `on_grant` only routes to a record that is still `Pending`, so sweeping
    /// first is what guarantees a grant delivered late (or redelivered inside
    /// the cursor lookback) cannot land on an ask that has already aged out.
    /// The reducer backs that up — every `(terminal, _)` pair falls through to
    /// the inert catch-all — but the check that actually fires is the `Pending`
    /// guard in `on_grant`, and this sweep is what makes it true in time.
    ///
    /// `Enacting` is deliberately untouched (see [`PENDING_TTL_SECS`]).
    fn expire_stale_pending(&self) {
        let now = self.now();
        // The effects are read inside the lock and CARRIED OUT outside it, so
        // neither a slow disk nor an event sink's channel ever holds the state
        // mutex.
        let expired = {
            let mut st = self.state.lock().expect("state lock");
            expire_stale_pending_records(&mut st.requests, now)
        };
        if expired.is_empty() {
            return;
        }
        for (key, rec, notify, persist) in expired {
            if persist {
                // Best-effort: a failed write leaves an on-disk record saying
                // Pending while memory says Expired. That is the safe way round
                // — memory is what `on_grant` consults, so the expiry still
                // holds for this run, and the next boot re-expires it by age.
                let _ = self.persist(&rec);
            }
            if notify {
                self.events.request_updated(&key, rec.state);
            }
        }
        // Expired is terminal, so the records just produced are now the cap's
        // business.
        self.prune_terminal();
    }

    /// Refuse a submit that is over this caller's outstanding or per-hour cap,
    /// and otherwise record the submit against the window.
    ///
    /// Called from [`Broker::submit_as`] BEFORE any id is generated, anything is
    /// persisted, or anything is published — a refused ask must leave no trace
    /// on disk, on the relay, or in the guardian's inbox, or the refusal would
    /// itself be the flood it is preventing.
    ///
    /// Both counts are keyed by `caller_uid` and only ever consulted for that
    /// key, so a sibling's flood cannot spend this caller's budget (nor the
    /// reverse). `None` — root, or a caller the bus could not identify — is its
    /// own key rather than a free pass.
    fn check_caller_caps(&self, caller_uid: Option<u32>, now: u64) -> Result<(), BrokerError> {
        let mut st = self.state.lock().expect("state lock");
        let outstanding = st
            .requests
            .values()
            .filter(|r| r.caller_uid == caller_uid && !r.state.is_terminal())
            .count();
        if outstanding >= MAX_PENDING_PER_CALLER {
            return Err(BrokerError::RateLimited(
                "too many requests waiting for your guardian — wait for an answer or try again \
                 later"
                    .into(),
            ));
        }
        let window_start = now.saturating_sub(SUBMIT_WINDOW_SECS);
        // Prune this caller's window WITHOUT creating an entry for a caller
        // that has none: `entry().or_default()` here would insert an empty
        // deque for every uid that ever asks, and the cleanup below could
        // never win against it.
        let pruned_len = match st.submits.get_mut(&caller_uid) {
            Some(stamps) => {
                while stamps.front().is_some_and(|t| *t < window_start) {
                    stamps.pop_front();
                }
                let len = stamps.len();
                // A deque pruned down to nothing is dead weight: without this,
                // every distinct `caller_uid` that has ever submitted keeps a
                // permanent entry in this map for the life of the process.
                // Dropped HERE and recreated only by a successful submit below
                // — the previous shape removed it and then immediately put it
                // back with an unconditional `or_default()`, so the cleanup
                // never actually removed anything and the map still grew
                // without bound.
                if len == 0 {
                    st.submits.remove(&caller_uid);
                }
                len
            }
            None => 0,
        };
        if pruned_len >= MAX_SUBMITS_PER_HOUR {
            return Err(BrokerError::RateLimited(
                "you've asked a lot recently — try again in a while".into(),
            ));
        }
        // Counted here rather than after a successful publish: a submit that
        // fails downstream still cost the work, and a caller who could retry a
        // failing op without limit is the same flood by another door. This is
        // the ONLY place an entry is created, so the map holds exactly those
        // callers with a live window.
        st.submits.entry(caller_uid).or_default().push_back(now);
        Ok(())
    }

    /// How many callers currently hold a non-empty submit window. Test-only:
    /// the rate-limiter's map is in-memory bookkeeping with no other observer,
    /// and "it does not grow without bound" needs one.
    #[cfg(test)]
    pub(crate) fn submit_window_callers(&self) -> usize {
        self.state.lock().expect("state lock").submits.len()
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
        // Per-caller caps FIRST: before an id exists, before anything is
        // written, before anything reaches the relay (03-G1).
        self.check_caller_caps(caller_uid, now)?;
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
        let _turn = self.handlers.lock().await;
        self.grant_disposition(received).await;
    }

    /// [`Broker::on_grant`]'s body, reporting what became of the grant. The
    /// caller holds the handler turn.
    async fn grant_disposition(&self, received: ReceivedGrant) -> GrantDisposition {
        // Route by the (unverified) reqId to find the pending record.
        let payload = match GrantPayload::from_json(&received.grant.content) {
            Ok(p) => p,
            Err(_) => return GrantDisposition::Malformed,
        };
        let key = payload.req_id.to_hex();
        let mut rec = {
            let st = self.state.lock().expect("state lock");
            match st.requests.get(&key) {
                Some(r) if r.state == RequestState::Pending => r.clone(),
                // Already resolved.
                Some(r) => return GrantDisposition::AlreadyResolved(r.state),
                None => return GrantDisposition::NoRecord,
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
        let mut rejection = None;
        let ev = match verify_grant(&received.grant, &vp, self.sys.consumed_ids()) {
            Ok(vg) => Event::GrantVerified(vg),
            Err(e) => {
                rejection = Some(GrantRejection::of(&e));
                Event::GrantRejected(e)
            }
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

        let resolved = rec.state;
        {
            let mut st = self.state.lock().expect("state lock");
            // `rec` was cloned from the LIVE map before any `.await` in this
            // function (verify/enact both suspend); a second delivery of the
            // same grant — or the TTL sweep expiring this same reqId — can run
            // to completion in that window and already have written a
            // resolved record back. Re-check the live entry before writing
            // back: if it is no longer `Pending`, someone else already
            // resolved it, and this (now-stale) computation must not clobber
            // that outcome.
            let still_pending = st
                .requests
                .get(&key)
                .is_some_and(|live| live.state == RequestState::Pending);
            if still_pending {
                st.requests.insert(key, rec);
            }
        }
        // A grant resolves the request to a terminal state (Enacted / Denied /
        // Failed); bound the retained terminal history.
        self.prune_terminal();
        match rejection {
            Some(r) => GrantDisposition::Rejected(r),
            None => GrantDisposition::Resolved(resolved),
        }
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
    /// Returns what became of the clause — the input to the
    /// `clausesAccepted` / `clausesWriteFailed` diagnostics. See
    /// [`ClauseOutcome`]: `Refused` means seen-but-refused (unparseable,
    /// wrong guardian, or below the replay floor), which is a very different
    /// fault from nothing arriving at all; `WriteFailed` means it WAS the
    /// guardian's clause and must not be reported as accepted.
    pub async fn on_clause(&self, received: ReceivedClause) -> ClauseOutcome {
        let _turn = self.handlers.lock().await;
        self.clause_disposition(&received).outcome()
    }

    /// [`Broker::on_clause`]'s body, in the detail an ACK needs. The caller
    /// holds the handler turn.
    fn clause_disposition(&self, received: &ReceivedClause) -> ClauseDisposition {
        let payload = match ClausePayload::from_json(&received.clause.content) {
            Ok(p) => p,
            Err(_) => return ClauseDisposition::Refused,
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
        // Per-child: rollback bound + cache are scoped to (subject, kind), so a
        // hostile relay cannot revert one child's clause or replay another
        // child's into theirs.
        let slot = match per_child_subject {
            Some(subject) => ClauseSlot::Child(subject.to_hex()),
            None => ClauseSlot::Machine,
        };

        // A sidecar floor (the record torn, or left below the sidecar by a
        // crash) takes the pinned guardian's re-send at the floor itself
        // (R2-7): `verify_bound` is one below it.
        let floor = match self.slot_floor(&slot, store_key) {
            Ok(floor) => floor,
            Err(e) => {
                warn_unreadable_floor(&slot.describe(store_key), e);
                return unreadable_floor_outcome(received, &pinned, payload.kind, now);
            }
        };
        let prev = floor.and_then(|f| f.verify_bound());
        match verify_clause(&received.clause, &pinned, payload.kind, prev, now) {
            Ok(vc) => {
                let body = serde_json::to_string(vc.body()).unwrap_or_default();
                match self.slot_put(&slot, store_key, vc.issued_at(), &body) {
                    Ok(true) => ClauseDisposition::Stored,
                    // The store's own floor refused it: authentic, but
                    // something at or above it is already there.
                    Ok(false) => ClauseDisposition::Superseded,
                    Err(e) => {
                        warn_write_failed(&slot.describe(store_key), e);
                        ClauseDisposition::WriteFailed
                    }
                }
            }
            // `verify_clause` checks the floor LAST, so a stale clause has
            // already authenticated as the pinned guardian's.
            Err(ClauseError::StaleIssuedAt) => {
                self.stale_clause_disposition(&slot, store_key, floor, &payload)
            }
            Err(_) => ClauseDisposition::Refused,
        }
    }

    /// An authentic clause at or below its slot's floor is `Unchanged` only
    /// when the slot holds exactly it: the stored record is at its `issuedAt`
    /// (a record floor — a sidecar floor means the record may never have
    /// landed) with an equal body. Anything else, including an unreadable
    /// record, is `Superseded`. The store keeps the body, not the event id, so
    /// equality is on what the device enforces (slot, `issuedAt`, body).
    fn stale_clause_disposition(
        &self,
        slot: &ClauseSlot,
        store_key: u16,
        floor: Option<ReplayFloor>,
        payload: &ClausePayload,
    ) -> ClauseDisposition {
        if floor != Some(ReplayFloor::Record(payload.issued_at)) {
            return ClauseDisposition::Superseded;
        }
        let held = match self.slot_get(slot, store_key) {
            Ok(Some(json)) => serde_json::from_str::<serde_json::Value>(&json).ok(),
            _ => None,
        };
        if held.as_ref() == Some(&payload.body) {
            ClauseDisposition::Unchanged
        } else {
            ClauseDisposition::Superseded
        }
    }

    fn slot_floor(&self, slot: &ClauseSlot, store_key: u16) -> SysResult<Option<ReplayFloor>> {
        match slot {
            ClauseSlot::Child(subject_hex) => self
                .sys
                .child_clauses()
                .replay_floor(subject_hex, store_key),
            ClauseSlot::Machine => self.sys.clauses().replay_floor(store_key),
        }
    }

    fn slot_get(&self, slot: &ClauseSlot, store_key: u16) -> SysResult<Option<String>> {
        match slot {
            ClauseSlot::Child(subject_hex) => self
                .sys
                .child_clauses()
                .get_child_clause(subject_hex, store_key),
            ClauseSlot::Machine => self.sys.clauses().get_clause(store_key),
        }
    }

    fn slot_put(
        &self,
        slot: &ClauseSlot,
        store_key: u16,
        issued_at: u64,
        body: &str,
    ) -> SysResult<bool> {
        match slot {
            ClauseSlot::Child(subject_hex) => {
                self.sys
                    .child_clauses()
                    .put_child_clause(subject_hex, store_key, issued_at, body)
            }
            ClauseSlot::Machine => self.sys.clauses().put_clause(store_key, issued_at, body),
        }
    }

    /// Handle a delivered (unverified) USAGE_SYNC: authenticate against the
    /// pinned guardian + per-subject `ts` replay floor, then cache the latest
    /// consolidated view in the per-(subject, key) store under
    /// [`USAGE_SYNC_STORE_KEY`]. The store's monotonic floor IS the replay
    /// protection, and `clear_for` on release wipes the view with the clauses.
    /// Staleness only under-counts the pool, so rejecting is always safe.
    pub async fn on_usage_sync(&self, event: charter_primitives::NostrEvent) {
        let _turn = self.handlers.lock().await;
        self.usage_sync_result(&event);
    }

    /// [`Broker::on_usage_sync`]'s body, reporting what became of the sync.
    /// The caller holds the handler turn.
    fn usage_sync_result(&self, event: &NostrEvent) -> AckResult {
        let Ok(payload) = UsageSyncPayload::from_json(&event.content) else {
            return AckResult::Refused;
        };
        let subject_hex = payload.subject.to_hex();
        let pinned = self.transport.pinned_guardian();
        let now = self.now();
        let prev = match self
            .sys
            .child_clauses()
            .highest_issued_at(&subject_hex, USAGE_SYNC_STORE_KEY)
        {
            Ok(prev) => prev,
            Err(e) => {
                warn_unreadable_floor(&format!("usage sync {subject_hex}"), e);
                // Never stored. Whether it was the guardian's decides only
                // what the ACK says (as for a clause over an unreadable floor).
                return match verify_usage_sync(event, &pinned, None, now) {
                    Ok(_) => AckResult::Failed,
                    Err(_) => AckResult::Refused,
                };
            }
        };
        match verify_usage_sync(event, &pinned, prev, now) {
            Ok(vs) => {
                // Same failure shape as an authenticated clause whose store write
                // fails (see `warn_write_failed`): this WAS the guardian's sync,
                // past the replay floor, but the write itself didn't land. Log it
                // rather than discard it — control flow is unchanged either way,
                // this is diagnostics only.
                match self.sys.child_clauses().put_child_clause(
                    &subject_hex,
                    USAGE_SYNC_STORE_KEY,
                    vs.ts(),
                    &vs.payload().to_json(),
                ) {
                    Ok(true) => AckResult::Applied,
                    Ok(false) => AckResult::Unchanged,
                    Err(e) => {
                        warn_write_failed(&format!("usage sync {subject_hex}"), e);
                        AckResult::Failed
                    }
                }
            }
            // Checked last, so an authentic sync: the device already holds
            // this view or a newer one.
            Err(UsageSyncError::StaleTs) => AckResult::Unchanged,
            Err(_) => AckResult::Refused,
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
        // Age out stale Pending records BEFORE fetching anything: a grant
        // delivered (or redelivered) this round must not be able to land on an
        // ask that is already past its TTL. See `expire_stale_pending`.
        self.expire_stale_pending();
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
        //
        // Each handler call below holds the handler turn (see `handlers`) for
        // that call only; the `poll_*` relay queries run outside it, so a
        // local-link ingest is never stuck behind relay IO.
        for (event, _seal_author) in self.transport.poll_releases(cursor, now).await {
            let applied = {
                let _turn = self.handlers.lock().await;
                self.on_release(&event, now).await
            };
            if applied {
                counts.released = true;
                return counts;
            }
        }
        for clause in self.transport.poll_clauses(cursor, now).await {
            counts.clauses_seen += 1;
            let outcome = {
                let _turn = self.handlers.lock().await;
                self.clause_disposition(&clause).outcome()
            };
            match outcome {
                ClauseOutcome::Stored => counts.clauses_accepted += 1,
                ClauseOutcome::WriteFailed => counts.clauses_write_failed += 1,
                ClauseOutcome::Refused => {}
            }
        }
        for (event, _seal_author) in self.transport.poll_usage_syncs(cursor, now).await {
            let _turn = self.handlers.lock().await;
            self.usage_sync_result(&event);
        }
        crate::curator_sync::refresh_curator_lists(&self.sys, &self.transport).await;
        for grant in self.transport.poll_grants(cursor, now).await {
            let _turn = self.handlers.lock().await;
            self.grant_disposition(grant).await;
        }
        let health = self.transport.poll_health();
        counts.relays_unreachable = health.relays_unreachable;
        counts.publish_failed = health.publish_failed;
        let mut st = self.state.lock().expect("state lock");
        // Lag the cursor a jitter window behind now (see `POLL_LOOKBACK_SECS`) so
        // a slightly-backdated or late-redelivered guardian event is never
        // permanently filtered out by the relay's `since` bound.
        st.cursor = now.saturating_sub(POLL_LOOKBACK_SECS);
        counts
    }

    /// Ingest one guardian gift wrap that arrived over the local link.
    ///
    /// The wrap is opened by the facade's [`TransportFacade::unwrap_local`]
    /// — the same NIP-59 unwrap as relay polling — and its inner event runs
    /// through the same handler as its relay copy would, holding the same
    /// handler turn: verification, pinning, freshness, nonce, consumed-id
    /// and floor rules are exactly the poll path's. Duplicates (the relay
    /// copy, a re-sent wrap) are absorbed by that same state, so whichever
    /// copy lands second changes nothing.
    ///
    /// The result says what this delivery did on the device, for the ward's
    /// signed ACK: see `docs/local-network-design.md`, "ACK".
    pub async fn ingest_local(
        &self,
        wrap: &NostrEvent,
        opts: LocalIngestOptions,
    ) -> LocalIngestOutcome {
        let mut out = LocalIngestOutcome::refused();
        let Some(inbound) = self.transport.unwrap_local(wrap, self.now()) else {
            return out;
        };
        match inbound {
            LocalInbound::Clause(clause) => {
                out.inner_kind = Some(clause.clause.kind);
                out.inner_id = Some(clause.clause.id);
                let _turn = self.handlers.lock().await;
                out.result = self.clause_disposition(&clause).ack();
            }
            LocalInbound::Grant(grant) => {
                out.inner_kind = Some(grant.grant.kind);
                out.inner_id = Some(grant.grant.id);
                out.req_id = GrantPayload::from_json(&grant.grant.content)
                    .ok()
                    .map(|p| p.req_id);
                let _turn = self.handlers.lock().await;
                // As at the top of `poll_once`: a grant must not land on an
                // ask that has already aged out.
                self.expire_stale_pending();
                out.result = self.grant_disposition(grant).await.ack();
            }
            LocalInbound::Release(event, seal_author) => {
                out.inner_kind = Some(event.kind);
                out.inner_id = Some(event.id);
                if opts.handle_release {
                    let _turn = self.handlers.lock().await;
                    if self.on_release(&event, self.now()).await {
                        out.result = AckResult::Applied;
                        out.released = true;
                    }
                } else {
                    out.result = AckResult::Ignored;
                    out.release = Some((event, seal_author));
                }
            }
            LocalInbound::UsageSync(event) => {
                out.inner_kind = Some(event.kind);
                out.inner_id = Some(event.id);
                let _turn = self.handlers.lock().await;
                out.result = self.usage_sync_result(&event);
            }
            LocalInbound::Other { inner_kind } => {
                out.inner_kind = Some(inner_kind);
                out.result = AckResult::Ignored;
            }
        }
        out
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
    fn pending_records_are_never_evicted_by_the_cap() {
        // Ten Pending records, cap of 3: none may be evicted (only terminal
        // records are counted against the bound). Pending records DO leave —
        // by ageing out through `PENDING_TTL_SECS`, which makes them Expired
        // and so terminal first (see `ttl_and_caps_tests`) — but never here,
        // and never while they are still Pending.
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

    /// The TTL rule itself, over the pure sweep: which states age out and which
    /// are left alone. The broker-level behaviour (when the sweep runs, and what
    /// a late grant then finds) is in `ttl_and_caps_tests`.
    #[test]
    fn only_pending_ages_out_and_only_once_past_the_ttl() {
        let born = 1_000_000u64;
        let mut map = BTreeMap::new();
        for (key, state) in [
            ("pending", RequestState::Pending),
            ("enacting", RequestState::Enacting),
            ("enacted", RequestState::Enacted),
        ] {
            map.insert(key.to_string(), rec(state, born));
        }

        // One second short of the TTL: nothing has aged out yet.
        let expired = expire_stale_pending_records(&mut map, born + PENDING_TTL_SECS - 1);
        assert!(expired.is_empty(), "the TTL has not elapsed yet");
        assert_eq!(map["pending"].state, RequestState::Pending);

        // Exactly at the TTL: the Pending record ages out, and ONLY it. An
        // Enacting record has a consumed grant and an enactor behind it — the
        // TTL must never race that.
        let expired = expire_stale_pending_records(&mut map, born + PENDING_TTL_SECS);
        assert_eq!(expired.len(), 1);
        let (key, record, notify, persist) = &expired[0];
        assert_eq!(key, "pending");
        assert_eq!(record.state, RequestState::Expired);
        assert!(
            *notify && *persist,
            "the reducer asks for both on an Expire"
        );
        assert_eq!(map["pending"].state, RequestState::Expired);
        assert_eq!(
            map["enacting"].state,
            RequestState::Enacting,
            "an Enacting record is never expired by the TTL"
        );
        assert_eq!(map["enacted"].state, RequestState::Enacted);

        // A second sweep has nothing left to do: Expired is terminal and inert.
        assert!(expire_stale_pending_records(&mut map, born + PENDING_TTL_SECS * 9).is_empty());
    }
}

/// 03-G1: the Pending TTL and the per-caller submit caps.
///
/// Before these, `SubmitRequest` was unlimited and a Pending record was
/// immortal: a ward looping `charter ask-for-more` grew charterd's map and the
/// on-disk `PendingStore` without bound, published a relay event per call into
/// their guardian's inbox, and flooded the event sink — a denial of the one
/// channel a ward has to be believed on.
#[cfg(all(test, feature = "mock"))]
mod ttl_and_caps_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use charter_sys::MockSystem;
    use charter_transport::ScriptedEntropy;
    use charter_verify::test_support::{GrantBuilder, TestGuardian};
    use charter_verify::VerifiedGrant;

    use crate::enactor::{EnactOutcome, Enactor};
    use crate::error::EnactError;
    use crate::ports::NullEventSink;
    use crate::transport_facade::MockTransport;

    const NOW: u64 = 1_700_001_000;
    const MIA: Option<u32> = Some(1000);
    const ROOK: Option<u32> = Some(1001);

    type TestBroker = Broker<MockSystem, MockTransport, ScriptedEntropy>;

    /// Counts every enact it is handed, so "an expired ask never enacts" can be
    /// asserted on the enactor rather than only on the record's state.
    struct CountingEnactor {
        enacts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Enactor for CountingEnactor {
        fn op(&self) -> OpType {
            OpType::InstallFlatpak
        }
        async fn enact(
            &self,
            _grant: &VerifiedGrant,
            _ctx: &EnactContext,
        ) -> Result<EnactOutcome, EnactError> {
            self.enacts.fetch_add(1, Ordering::SeqCst);
            Ok(EnactOutcome::default())
        }
    }

    fn broker_over(sys: MockSystem, guardian: &TestGuardian) -> (TestBroker, Arc<AtomicUsize>) {
        let enacts = Arc::new(AtomicUsize::new(0));
        let mut reg = EnactorRegistry::new();
        reg.register(Box::new(CountingEnactor {
            enacts: enacts.clone(),
        }));
        let b = Broker::new(
            sys,
            MockTransport::new(guardian.pubkey(), PubKey::from_bytes([0x42; 32])),
            ScriptedEntropy::new(1),
            reg,
            Box::new(NullEventSink),
            PubKey::from_bytes([0xBB; 32]),
        );
        (b, enacts)
    }

    fn broker(guardian: &TestGuardian) -> (TestBroker, Arc<AtomicUsize>) {
        broker_over(MockSystem::new(NOW), guardian)
    }

    async fn ask(b: &TestBroker, uid: Option<u32>) -> Result<ReqId, BrokerError> {
        b.submit_as(
            OpType::TimeExtend,
            serde_json::json!({"minutes": 10}),
            None,
            uid,
        )
        .await
    }

    fn state_of(b: &TestBroker, hex: &str) -> RequestState {
        b.status(hex).pop().expect("record").state
    }

    fn assert_rate_limited(e: BrokerError, expect: &str) {
        match e {
            BrokerError::RateLimited(msg) => assert!(
                msg.contains(expect),
                "expected a message containing {expect:?}, got {msg:?}"
            ),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_pending_ask_expires_once_past_its_ttl_and_not_before() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        let hex = ask(&b, MIA).await.expect("submitted").to_hex();

        // One second short of a day: still waiting. A guardian who looks at it
        // over breakfast must still find it there.
        b.sys().mock_clock().advance_secs(PENDING_TTL_SECS - 1);
        b.poll_once().await;
        assert_eq!(state_of(&b, &hex), RequestState::Pending);

        // Past the TTL: aged out, in memory and on disk.
        b.sys().mock_clock().advance_secs(1);
        b.poll_once().await;
        assert_eq!(state_of(&b, &hex), RequestState::Expired);
        let stored = b.sys().pending().list().expect("list");
        let (_, json) = stored.iter().find(|(k, _)| *k == hex).expect("persisted");
        let rec: RequestRecord = serde_json::from_str(json).expect("parse");
        assert_eq!(
            rec.state,
            RequestState::Expired,
            "the expiry must reach the persisted store, not just memory"
        );
    }

    #[tokio::test]
    async fn a_record_loaded_older_than_the_ttl_comes_up_expired() {
        // A daemon that was off for a week must not come back up holding a
        // week-old ask as live.
        let g = TestGuardian::new();
        let (first, _) = broker(&g);
        let hex = ask(&first, MIA).await.expect("submitted").to_hex();
        let disk = first.sys().disk();
        assert_eq!(state_of(&first, &hex), RequestState::Pending);
        drop(first);

        // The daemon comes back up a week later over the SAME disk. The ask must
        // already be Expired by the time anything can poll — the boot sweep runs
        // inside `Broker::new`, before a grant could ever be matched to it.
        let sys = MockSystem::over_disk(NOW + PENDING_TTL_SECS + 7 * 86_400, disk);
        let (rebooted, _) = broker_over(sys, &g);
        assert_eq!(
            state_of(&rebooted, &hex),
            RequestState::Expired,
            "a week-old ask must not come back up live"
        );
    }

    #[tokio::test]
    async fn a_grant_for_an_expired_ask_never_enacts() {
        let g = TestGuardian::new();
        let (b, enacts) = broker(&g);
        let req = b
            .submit(
                OpType::InstallFlatpak,
                serde_json::json!({"ref": "org.req.X", "remote": "flathub"}),
                None,
            )
            .await
            .expect("submitted");
        let hex = req.to_hex();
        let rec = b.status(&hex).pop().expect("record");

        // The ask ages out, and only THEN does the guardian's grant arrive —
        // the late-delivery case a relay's store-and-forward makes routine.
        b.sys().mock_clock().advance_secs(PENDING_TTL_SECS + 60);
        let grant = GrantBuilder::install_allow(rec.req_id, rec.nonce)
            .params(serde_json::json!({"ref": "org.grant.App", "remote": "flathub"}))
            .build(&g);
        b.transport().deliver_grant(grant, g.pubkey());
        b.poll_once().await;

        assert_eq!(
            state_of(&b, &hex),
            RequestState::Expired,
            "a late grant must not revive an expired ask"
        );
        assert_eq!(
            enacts.load(Ordering::SeqCst),
            0,
            "an expired ask must never enact"
        );
    }

    #[tokio::test]
    async fn a_callers_outstanding_asks_are_capped_but_a_siblings_are_their_own() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        let mut hexes = Vec::new();
        for _ in 0..MAX_PENDING_PER_CALLER {
            hexes.push(ask(&b, MIA).await.expect("under the cap").to_hex());
        }

        let published = b.transport().published().len();
        let err = ask(&b, MIA).await.expect_err("over the outstanding cap");
        assert_rate_limited(err, "waiting for your guardian");
        assert_eq!(
            b.transport().published().len(),
            published,
            "a refused ask must never reach the relay"
        );
        assert_eq!(
            b.status("").len(),
            MAX_PENDING_PER_CALLER,
            "a refused ask must leave no record behind"
        );

        // One child's flood does not spend their sibling's budget.
        ask(&b, ROOK).await.expect("a sibling is unaffected");
        // Nor root's / an unidentified caller's, which is its own key.
        ask(&b, None).await.expect("None is its own key");

        // A guardian answering one (here: the ward withdrawing it) frees a slot.
        assert!(b.cancel(&hexes[0]));
        ask(&b, MIA).await.expect("a freed slot is usable again");
    }

    #[tokio::test]
    async fn the_submit_rate_is_capped_per_hour_and_the_window_slides() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        // Withdraw each ask straight away, so the OUTSTANDING cap never binds
        // and what is under test is purely the hourly rate.
        for _ in 0..MAX_SUBMITS_PER_HOUR {
            let hex = ask(&b, MIA).await.expect("under the rate").to_hex();
            assert!(b.cancel(&hex));
        }

        let published = b.transport().published().len();
        let err = ask(&b, MIA).await.expect_err("over the hourly rate");
        assert_rate_limited(err, "asked a lot recently");
        assert_eq!(
            b.transport().published().len(),
            published,
            "a rate-refused ask must never reach the relay"
        );
        // A sibling's budget is untouched by this one's spending.
        ask(&b, ROOK).await.expect("a sibling is unaffected");

        // The window slides: once the oldest submits fall out of the hour, the
        // caller may ask again.
        b.sys().mock_clock().advance_secs(SUBMIT_WINDOW_SECS + 1);
        ask(&b, MIA).await.expect("the window has slid");
    }

    /// The rate-limiter's bookkeeping must not be a slow leak. A caller whose
    /// whole window has expired keeps no entry in the map — and a caller who is
    /// merely *asked about* (a refused submit, a sibling's check) never creates
    /// one at all. The previous shape removed the empty deque and then put it
    /// straight back with an unconditional `or_default()`, so every uid that
    /// ever touched the broker stayed in the map for the life of the process.
    #[tokio::test]
    async fn an_expired_submit_window_leaves_no_entry_behind() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        let hex = ask(&b, MIA).await.expect("under the rate").to_hex();
        assert!(b.cancel(&hex));
        assert_eq!(b.submit_window_callers(), 1, "one caller has a live window");

        // The window expires, and the next thing that asks about this caller
        // prunes it away rather than renewing an empty entry.
        b.sys().mock_clock().advance_secs(SUBMIT_WINDOW_SECS + 1);
        let hex = ask(&b, MIA).await.expect("the window has slid").to_hex();
        assert!(b.cancel(&hex));
        assert_eq!(
            b.submit_window_callers(),
            1,
            "the expired window was replaced, not accumulated"
        );
    }

    /// Relay health wiring (02b-G1/02b-B5 follow-through): a transport whose
    /// query reported every relay unreachable this round must surface that on
    /// `PollCounts`, and nothing else about the round changes.
    #[tokio::test]
    async fn poll_once_reports_relays_unreachable_and_nothing_else_changes() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        b.transport().set_relays_unreachable(true);
        let counts = b.poll_once().await;
        assert!(counts.relays_unreachable);
        assert_eq!(counts.publish_failed, 0);
        assert_eq!(counts.clauses_seen, 0);
        assert_eq!(counts.clauses_accepted, 0);
        assert_eq!(counts.clauses_write_failed, 0);
        assert!(!counts.released);

        // The signal is read-and-reset: a healthy round right after reports
        // no lingering unreachable flag.
        let counts2 = b.poll_once().await;
        assert!(!counts2.relays_unreachable);
    }

    /// A publish that failed against one relay this round must be counted.
    #[tokio::test]
    async fn poll_once_counts_a_single_relay_publish_failure() {
        let g = TestGuardian::new();
        let (b, _) = broker(&g);
        b.transport().set_publish_failed(1);
        let counts = b.poll_once().await;
        assert_eq!(counts.publish_failed, 1);
        assert!(!counts.relays_unreachable);
    }
}
