//! The guardian role: a TCP listener on the carrier that wards connect to.
//!
//! The listener proves the guardian key to anyone who says HELLO (the key is
//! already public on relays), admits only machine keys in the allowed set,
//! and then exchanges gift wraps. It never interprets a wrap: inbound wraps
//! go to a persisted inbox for the console's normal ingest, outbound wraps
//! are written to a live session or queued until the ward connects.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener as StdListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use charter_primitives::{EventId, NostrEvent, PubKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::proto::{
    guardian_proof_sign, peer_address_allowed, public_key, validate_wrap_for, verify_ack,
    ward_proof_verify, Frame, PROTOCOL_VERSION,
};
use crate::session::{
    challenge, read_frame, send_frame, write_frame, writer, Closer, RateLimit, HANDSHAKE_TIMEOUT,
    IDLE_TIMEOUT,
};
use crate::store::{load, lock, now_unix, save_logged};
use crate::AckResult;

const MAX_UNAUTHENTICATED: usize = 8;
const MAX_SESSIONS: usize = 16;
// Matches the ward's own session cap, so a carrier reachable at several
// addresses never evicts and re-admits the same ward in a loop.
const MAX_SESSIONS_PER_MACHINE: usize = 4;
const QUEUE_PER_MACHINE: usize = 64;
const QUEUE_TTL: u64 = 48 * 3600;
const DELIVERY_CAP: usize = 512;
const DELIVERY_TTL: u64 = 7 * 24 * 3600;
const INBOX_CAP: usize = 256;
const SEEN_CAP: usize = 1024;
const SEEN_TTL: u64 = 48 * 3600;
const EVENTS_CAP: usize = 1024;
const SESSION_CHANNEL: usize = 256;

const OUTBOUND_FILE: &str = "link-outbound.json";
const DELIVERY_FILE: &str = "link-delivery.json";
const INBOX_FILE: &str = "link-inbox.json";
const SEEN_FILE: &str = "link-seen.json";

/// Configuration for [`GuardianLink::start`].
pub struct GuardianLinkConfig {
    pub guardian_sk: Zeroizing<[u8; 32]>,
    pub state_dir: PathBuf,
    pub preferred_port: u16,
    pub allow_loopback: bool,
}

impl std::fmt::Debug for GuardianLinkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardianLinkConfig")
            .field("guardian_sk", &"<redacted>")
            .field("state_dir", &self.state_dir)
            .field("preferred_port", &self.preferred_port)
            .field("allow_loopback", &self.allow_loopback)
            .finish()
    }
}

/// The outcome of [`GuardianLink::send`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendState {
    /// Written to an authenticated session with the recipient.
    Sent,
    /// Held until the recipient connects.
    Queued,
    /// Not sent and not queued, with the reason.
    Rejected(String),
}

impl SendState {
    /// The bridge spelling: `sent`, `queued` or `rejected`.
    pub fn as_str(&self) -> &'static str {
        match self {
            SendState::Sent => "sent",
            SendState::Queued => "queued",
            SendState::Rejected(_) => "rejected",
        }
    }
}

impl Serialize for SendState {
    /// `{"state":"sent"|"queued"|"rejected","reason"?:…}`, the bridge shape.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("state", self.as_str())?;
        if let SendState::Rejected(r) = self {
            m.serialize_entry("reason", r)?;
        }
        m.end()
    }
}

/// Where one guardian wrap stands with its ward device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryState {
    Queued,
    Sent,
    Received,
    Rejected,
    Acked(AckResult),
}

impl DeliveryState {
    /// The bridge spelling: `queued`, `sent`, `received`, `rejected`, or the
    /// ACK result (`applied`, `unchanged`, …).
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryState::Queued => "queued",
            DeliveryState::Sent => "sent",
            DeliveryState::Received => "received",
            DeliveryState::Rejected => "rejected",
            DeliveryState::Acked(r) => r.as_str(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => DeliveryState::Queued,
            "sent" => DeliveryState::Sent,
            "received" => DeliveryState::Received,
            "rejected" => DeliveryState::Rejected,
            other => DeliveryState::Acked(AckResult::parse(other)?),
        })
    }

    /// Whether `next` should replace `self`. States only move forward, a
    /// later ACK replaces only a `failed` one, and repeats are no-ops.
    fn advanced_by(self, next: DeliveryState) -> bool {
        use DeliveryState::*;
        if self == next {
            return false;
        }
        match (self, next) {
            (Acked(AckResult::Failed), Acked(_)) => true,
            (Acked(_), _) => false,
            (_, Acked(_)) => true,
            (Queued | Sent, Received | Rejected) => true,
            (Received, Rejected) | (Rejected, Received) => true,
            (Queued, Sent) => true,
            _ => false,
        }
    }
}

impl Serialize for DeliveryState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DeliveryState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DeliveryState::parse(&s).ok_or_else(|| serde::de::Error::custom("unknown delivery state"))
    }
}

/// The latest delivery state of one wrap, as the bridge reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub state: DeliveryState,
    /// Unix seconds of the last change.
    pub at: u64,
    pub machine: PubKey,
}

/// A ward wrap held for the console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    pub id: EventId,
    /// The authenticated machine key of the session it arrived on.
    pub machine: PubKey,
    pub wrap: NostrEvent,
    pub received_at: u64,
}

/// One authenticated ward session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub machine: PubKey,
    /// Unix seconds when the session became ready.
    pub since: u64,
}

/// What [`GuardianLink::next_events`] reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum GuardianEvent {
    /// A new (not previously seen) ward wrap, also stored in the inbox.
    Wrap { machine: PubKey, wrap: NostrEvent },
    /// A guardian wrap's delivery state changed.
    Delivery {
        wrap: EventId,
        machine: PubKey,
        state: DeliveryState,
    },
    /// A ward device came onto (`up`) or left the home network: its first
    /// session opened or its last one closed.
    Session { machine: PubKey, up: bool },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Queued {
    machine: PubKey,
    wrap: NostrEvent,
    queued_at: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct SeenId {
    id: EventId,
    at: u64,
}

#[derive(Serialize, Deserialize)]
struct QueueFile {
    v: u8,
    queue: Vec<Queued>,
}

#[derive(Serialize, Deserialize)]
struct DeliveryFile {
    v: u8,
    records: Vec<DeliveryEntry>,
}

#[derive(Serialize, Deserialize)]
struct DeliveryEntry {
    wrap: EventId,
    #[serde(flatten)]
    record: DeliveryRecord,
}

#[derive(Serialize, Deserialize)]
struct InboxFile {
    v: u8,
    entries: Vec<InboxEntry>,
}

#[derive(Serialize, Deserialize)]
struct SeenFile {
    v: u8,
    seen: Vec<SeenId>,
}

struct Session {
    machine: PubKey,
    since: u64,
    started: Instant,
    tx: mpsc::Sender<Vec<u8>>,
    closer: Arc<Closer>,
}

struct State {
    closed: bool,
    /// False while the carrier is off any home network: connections are
    /// refused, sessions closed; queue and inbox are kept.
    accepting: bool,
    allowed: HashSet<PubKey>,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    queue: Vec<Queued>,
    delivery: BTreeMap<EventId, DeliveryRecord>,
    inbox: Vec<InboxEntry>,
    seen: Vec<SeenId>,
}

struct Inner {
    sk: Zeroizing<[u8; 32]>,
    guardian: PubKey,
    dir: PathBuf,
    allow_loopback: bool,
    state: Mutex<State>,
    events: Mutex<VecDeque<GuardianEvent>>,
    events_cv: Condvar,
    unauthenticated: AtomicUsize,
    shutdown: Arc<Closer>,
}

/// The guardian carrier's local link listener.
pub struct GuardianLink {
    inner: Arc<Inner>,
    port: u16,
}

impl GuardianLink {
    /// Bind `0.0.0.0:preferred_port` (else an ephemeral port), load the
    /// persisted queue, delivery records, inbox and seen set from
    /// `state_dir`, and start accepting on `rt`.
    pub fn start(
        cfg: GuardianLinkConfig,
        allowed: Vec<PubKey>,
        rt: &tokio::runtime::Handle,
    ) -> io::Result<Arc<Self>> {
        let guardian = public_key(&cfg.guardian_sk)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid guardian key"))?;
        std::fs::create_dir_all(&cfg.state_dir)?;
        let dir = cfg.state_dir.clone();
        let now = now_unix();

        let mut state = State {
            closed: false,
            accepting: true,
            allowed: allowed.into_iter().collect(),
            sessions: HashMap::new(),
            next_session: 1,
            queue: load::<QueueFile>(&dir.join(OUTBOUND_FILE))
                .map(|f| f.queue)
                .unwrap_or_default(),
            delivery: load::<DeliveryFile>(&dir.join(DELIVERY_FILE))
                .map(|f| f.records.into_iter().map(|e| (e.wrap, e.record)).collect())
                .unwrap_or_default(),
            inbox: load::<InboxFile>(&dir.join(INBOX_FILE))
                .map(|f| f.entries)
                .unwrap_or_default(),
            seen: load::<SeenFile>(&dir.join(SEEN_FILE))
                .map(|f| f.seen)
                .unwrap_or_default(),
        };
        prune_queue(&mut state.queue, now);
        prune_delivery(&mut state.delivery, now);
        prune_seen(&mut state.seen, now);

        let std_listener = bind(cfg.preferred_port)?;
        std_listener.set_nonblocking(true)?;
        let port = std_listener.local_addr()?.port();
        let listener = {
            let _enter = rt.enter();
            TcpListener::from_std(std_listener)?
        };

        let inner = Arc::new(Inner {
            sk: cfg.guardian_sk,
            guardian,
            dir,
            allow_loopback: cfg.allow_loopback,
            state: Mutex::new(state),
            events: Mutex::new(VecDeque::new()),
            events_cv: Condvar::new(),
            unauthenticated: AtomicUsize::new(0),
            shutdown: Arc::new(Closer::default()),
        });
        rt.spawn(accept_loop(inner.clone(), listener));
        eprintln!("charter-link: guardian listening on port {port}");
        Ok(Arc::new(GuardianLink { inner, port }))
    }

    /// The bound TCP port (to advertise over DNS-SD).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Replace the set of machine keys admitted at the handshake, closing
    /// sessions whose machine is no longer allowed.
    pub fn set_allowed(&self, allowed: Vec<PubKey>) {
        let mut st = lock(&self.inner.state);
        st.allowed = allowed.into_iter().collect();
        let gone: Vec<u64> = st
            .sessions
            .iter()
            .filter(|(_, s)| !st.allowed.contains(&s.machine))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.inner.drop_session(&mut st, id);
        }
    }

    /// Accept ward connections (`true`, the default) or refuse them while the
    /// carrier has no home network (`false`): new connections close at once
    /// and every session closes. Queued wraps, the inbox and delivery records
    /// are kept.
    pub fn set_accepting(&self, on: bool) {
        let mut st = lock(&self.inner.state);
        st.accepting = on;
        if !on {
            let all: Vec<u64> = st.sessions.keys().copied().collect();
            for id in all {
                self.inner.drop_session(&mut st, id);
            }
        }
    }

    /// Send a wrap to a ward: written at once to a live session, otherwise
    /// queued (persisted) until the ward connects.
    pub fn send(&self, recipient: &PubKey, wrap: NostrEvent) -> SendState {
        if let Err(e) = validate_wrap_for(&wrap, recipient) {
            return SendState::Rejected(e.to_string());
        }
        let inner = &self.inner;
        let mut st = lock(&inner.state);
        if st.closed {
            return SendState::Rejected("link stopped".into());
        }
        if !st.allowed.contains(recipient) {
            return SendState::Rejected("recipient is not a paired device".into());
        }
        let frame = crate::proto::encode_frame(&Frame::Event(wrap.clone()));
        let newest = st
            .sessions
            .values()
            .filter(|s| s.machine == *recipient && !s.closer.is_closed())
            .max_by_key(|s| s.started);
        let sent = newest.is_some_and(|s| s.tx.try_send(frame).is_ok());
        if sent {
            inner.set_delivery(&mut st, wrap.id, *recipient, DeliveryState::Sent);
            inner.persist_delivery(&st);
            return SendState::Sent;
        }
        let now = now_unix();
        if !st
            .queue
            .iter()
            .any(|q| q.machine == *recipient && q.wrap.id == wrap.id)
        {
            st.queue.push(Queued {
                machine: *recipient,
                wrap: wrap.clone(),
                queued_at: now,
            });
        }
        prune_queue(&mut st.queue, now);
        inner.persist_queue(&st);
        inner.set_delivery(&mut st, wrap.id, *recipient, DeliveryState::Queued);
        inner.persist_delivery(&st);
        SendState::Queued
    }

    /// Wait up to `timeout` for events and return all that are pending.
    /// Blocking: meant for a dedicated (non-async) thread. Returns early,
    /// possibly empty, after [`GuardianLink::shutdown`].
    pub fn next_events(&self, timeout: Duration) -> Vec<GuardianEvent> {
        let deadline = std::time::Instant::now() + timeout;
        let mut q = lock(&self.inner.events);
        while q.is_empty() && !self.inner.shutdown.is_closed() {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            q = match self.inner.events_cv.wait_timeout(q, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
        q.drain(..).collect()
    }

    /// Ward wraps held for the console, oldest first.
    pub fn inbox(&self) -> Vec<InboxEntry> {
        lock(&self.inner.state).inbox.clone()
    }

    /// Drop inbox entries the console has ingested. Their ids stay in the
    /// seen set, so a later duplicate is not stored again.
    pub fn consume(&self, ids: &[EventId]) {
        let mut st = lock(&self.inner.state);
        let before = st.inbox.len();
        st.inbox.retain(|e| !ids.contains(&e.id));
        if st.inbox.len() != before {
            self.inner.persist_inbox(&st);
        }
    }

    /// The delivery records for those of `ids` this link has handled.
    pub fn delivery(&self, ids: &[EventId]) -> BTreeMap<EventId, DeliveryRecord> {
        let st = lock(&self.inner.state);
        ids.iter()
            .filter_map(|id| st.delivery.get(id).map(|r| (*id, r.clone())))
            .collect()
    }

    /// Authenticated sessions, oldest first.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let st = lock(&self.inner.state);
        let mut v: Vec<(&Instant, SessionInfo)> = st
            .sessions
            .values()
            .map(|s| {
                (
                    &s.started,
                    SessionInfo {
                        machine: s.machine,
                        since: s.since,
                    },
                )
            })
            .collect();
        v.sort_by_key(|(t, _)| **t);
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// Stop listening and close every connection. Persisted state is left
    /// as of this call; a new link may start on the same directory at once.
    pub fn shutdown(&self) {
        {
            let mut st = lock(&self.inner.state);
            if st.closed {
                return;
            }
            st.closed = true;
            for s in st.sessions.values() {
                s.closer.close();
            }
            st.sessions.clear();
        }
        self.inner.shutdown.close();
        let _q = lock(&self.inner.events);
        self.inner.events_cv.notify_all();
    }
}

impl Drop for GuardianLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn bind(preferred: u16) -> io::Result<StdListener> {
    if preferred != 0 {
        if let Ok(l) = StdListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, preferred)) {
            return Ok(l);
        }
    }
    StdListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

fn prune_queue(q: &mut Vec<Queued>, now: u64) {
    q.retain(|e| now.saturating_sub(e.queued_at) < QUEUE_TTL);
    let mut per: HashMap<PubKey, usize> = HashMap::new();
    for e in q.iter() {
        *per.entry(e.machine).or_default() += 1;
    }
    for (m, n) in per {
        if n > QUEUE_PER_MACHINE {
            let mut drop = n - QUEUE_PER_MACHINE;
            q.retain(|e| {
                if drop > 0 && e.machine == m {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }
}

fn prune_delivery(d: &mut BTreeMap<EventId, DeliveryRecord>, now: u64) {
    d.retain(|_, r| now.saturating_sub(r.at) < DELIVERY_TTL);
    while d.len() > DELIVERY_CAP {
        let oldest = d.iter().min_by_key(|(_, r)| r.at).map(|(id, _)| *id);
        match oldest {
            Some(id) => {
                d.remove(&id);
            }
            None => break,
        }
    }
}

fn prune_seen(s: &mut Vec<SeenId>, now: u64) {
    s.retain(|e| now.saturating_sub(e.at) < SEEN_TTL);
    if s.len() > SEEN_CAP {
        let excess = s.len() - SEEN_CAP;
        s.drain(..excess);
    }
}

impl Inner {
    fn emit(&self, ev: GuardianEvent) {
        let mut q = lock(&self.events);
        q.push_back(ev);
        while q.len() > EVENTS_CAP {
            q.pop_front();
        }
        self.events_cv.notify_all();
    }

    fn set_delivery(&self, st: &mut State, wrap: EventId, machine: PubKey, state: DeliveryState) {
        let changed = match st.delivery.get(&wrap) {
            Some(r) if r.machine == machine => r.state.advanced_by(state),
            Some(_) => false,
            None => true,
        };
        if !changed {
            return;
        }
        let now = now_unix();
        st.delivery.insert(
            wrap,
            DeliveryRecord {
                state,
                at: now,
                machine,
            },
        );
        prune_delivery(&mut st.delivery, now);
        self.emit(GuardianEvent::Delivery {
            wrap,
            machine,
            state,
        });
    }

    fn persist_queue(&self, st: &State) {
        if !st.closed {
            save_logged(
                &self.dir.join(OUTBOUND_FILE),
                &QueueFile {
                    v: 1,
                    queue: st.queue.clone(),
                },
            );
        }
    }

    fn persist_delivery(&self, st: &State) {
        if !st.closed {
            let records = st
                .delivery
                .iter()
                .map(|(id, r)| DeliveryEntry {
                    wrap: *id,
                    record: r.clone(),
                })
                .collect();
            save_logged(
                &self.dir.join(DELIVERY_FILE),
                &DeliveryFile { v: 1, records },
            );
        }
    }

    fn persist_inbox(&self, st: &State) {
        if !st.closed {
            save_logged(
                &self.dir.join(INBOX_FILE),
                &InboxFile {
                    v: 1,
                    entries: st.inbox.clone(),
                },
            );
        }
    }

    fn persist_seen(&self, st: &State) {
        if !st.closed {
            save_logged(
                &self.dir.join(SEEN_FILE),
                &SeenFile {
                    v: 1,
                    seen: st.seen.clone(),
                },
            );
        }
    }

    /// Remove and close a session, reporting the machine as gone if it was
    /// its last session.
    fn drop_session(&self, st: &mut State, id: u64) {
        if let Some(s) = st.sessions.remove(&id) {
            s.closer.close();
            if !st.sessions.values().any(|o| o.machine == s.machine) {
                self.emit(GuardianEvent::Session {
                    machine: s.machine,
                    up: false,
                });
            }
        }
    }
}

async fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    loop {
        let accepted = tokio::select! {
            _ = inner.shutdown.closed() => break,
            r = listener.accept() => r,
        };
        let (stream, peer) = match accepted {
            Ok(x) => x,
            Err(e) => {
                eprintln!("charter-link: accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if !peer_address_allowed(peer.ip(), inner.allow_loopback) || !lock(&inner.state).accepting {
            drop(stream);
            continue;
        }
        if inner.unauthenticated.fetch_add(1, Ordering::SeqCst) >= MAX_UNAUTHENTICATED {
            inner.unauthenticated.fetch_sub(1, Ordering::SeqCst);
            drop(stream);
            continue;
        }
        tokio::spawn(connection(inner.clone(), stream, peer));
    }
}

/// Decrements the unauthenticated count when the handshake ends either way.
struct Unauthenticated<'a>(&'a AtomicUsize);

impl Drop for Unauthenticated<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn connection(inner: Arc<Inner>, stream: TcpStream, peer: SocketAddr) {
    let _ = stream.set_nodelay(true);
    let (mut rd, mut wr) = stream.into_split();
    let mut rate = RateLimit::new();
    let machine = {
        let _slot = Unauthenticated(&inner.unauthenticated);
        let hs = handshake(&inner, &mut rd, &mut wr, &mut rate);
        let r = tokio::select! {
            _ = inner.shutdown.closed() => None,
            r = tokio::time::timeout(HANDSHAKE_TIMEOUT, hs) => r.ok().flatten(),
        };
        match r {
            Some(m) => m,
            None => return,
        }
    };

    let closer = Arc::new(Closer::default());
    let (tx, rx) = mpsc::channel::<Vec<u8>>(SESSION_CHANNEL);
    let id = {
        let mut st = lock(&inner.state);
        if st.closed
            || !st.accepting
            || !st.allowed.contains(&machine)
            || st.sessions.len() >= MAX_SESSIONS
        {
            return;
        }
        let mut mine: Vec<(Instant, u64)> = st
            .sessions
            .iter()
            .filter(|(_, s)| s.machine == machine)
            .map(|(id, s)| (s.started, *id))
            .collect();
        mine.sort();
        while mine.len() >= MAX_SESSIONS_PER_MACHINE {
            let (_, oldest) = mine.remove(0);
            if let Some(s) = st.sessions.remove(&oldest) {
                s.closer.close();
            }
        }
        // READY goes first on the channel, before the session is visible to
        // `send`, so nothing can overtake it.
        if tx
            .try_send(crate::proto::encode_frame(&Frame::Ready {
                v: PROTOCOL_VERSION as u64,
            }))
            .is_err()
        {
            return;
        }
        let first = !st.sessions.values().any(|s| s.machine == machine);
        let id = st.next_session;
        st.next_session += 1;
        st.sessions.insert(
            id,
            Session {
                machine,
                since: now_unix(),
                started: Instant::now(),
                tx: tx.clone(),
                closer: closer.clone(),
            },
        );
        if first {
            inner.emit(GuardianEvent::Session { machine, up: true });
        }
        id
    };
    eprintln!("charter-link: ward session up from {}", peer.ip());
    let writer_task = tokio::spawn(writer(wr, rx, closer.clone()));

    tokio::select! {
        _ = inner.shutdown.closed() => {}
        _ = closer.closed() => {}
        _ = async {
            if flush_queue(&inner, &machine, &tx).await {
                session(&inner, machine, &mut rd, &tx, &mut rate).await;
            }
        } => {}
    }

    closer.close();
    writer_task.abort();
    let mut st = lock(&inner.state);
    inner.drop_session(&mut st, id);
    drop(st);
    eprintln!("charter-link: ward session closed from {}", peer.ip());
}

/// HELLO → CHALLENGE → AUTH. Returns the authenticated machine key.
async fn handshake(
    inner: &Inner,
    rd: &mut tokio::net::tcp::OwnedReadHalf,
    wr: &mut tokio::net::tcp::OwnedWriteHalf,
    rate: &mut RateLimit,
) -> Option<PubKey> {
    let cw = match read_frame(rd).await.ok()? {
        Frame::Hello { v, challenge } if v == PROTOCOL_VERSION as u64 => challenge,
        _ => return None,
    };
    rate.hit();
    let cg = challenge().ok()?;
    let sig = guardian_proof_sign(&inner.sk, &cw, &cg)?;
    write_frame(
        wr,
        &Frame::Challenge {
            v: PROTOCOL_VERSION as u64,
            guardian: inner.guardian,
            challenge: cg,
            sig,
        },
    )
    .await
    .ok()?;
    let (machine, sig) = match read_frame(rd).await.ok()? {
        Frame::Auth { machine, sig } => (machine, sig),
        _ => return None,
    };
    rate.hit();
    let allowed = lock(&inner.state).allowed.contains(&machine);
    if !allowed || !ward_proof_verify(&cw, &cg, &inner.guardian, &machine, &sig) {
        return None;
    }
    Some(machine)
}

/// Write every queued wrap for `machine`, oldest first. Entries stay queued
/// until the ward's `OK`, so a session that drops mid-flush loses nothing.
async fn flush_queue(inner: &Inner, machine: &PubKey, tx: &mpsc::Sender<Vec<u8>>) -> bool {
    let wraps: Vec<NostrEvent> = {
        let mut st = lock(&inner.state);
        prune_queue(&mut st.queue, now_unix());
        st.queue
            .iter()
            .filter(|q| q.machine == *machine)
            .map(|q| q.wrap.clone())
            .collect()
    };
    for wrap in wraps {
        let id = wrap.id;
        if !send_frame(tx, &Frame::Event(wrap)).await {
            return false;
        }
        let mut st = lock(&inner.state);
        inner.set_delivery(&mut st, id, *machine, DeliveryState::Sent);
        inner.persist_delivery(&st);
    }
    true
}

/// The authenticated session's inbound loop.
async fn session(
    inner: &Inner,
    machine: PubKey,
    rd: &mut tokio::net::tcp::OwnedReadHalf,
    tx: &mpsc::Sender<Vec<u8>>,
    rate: &mut RateLimit,
) {
    loop {
        let frame = match tokio::time::timeout(IDLE_TIMEOUT, read_frame(rd)).await {
            Ok(Ok(f)) => f,
            _ => return,
        };
        if !rate.hit() {
            eprintln!("charter-link: ward session over its frame rate; closing");
            return;
        }
        let reply = match frame {
            Frame::Event(wrap) => Some(inbound_wrap(inner, machine, wrap)),
            Frame::Ok { wrap, accepted, .. } => {
                let mut st = lock(&inner.state);
                if st.delivery.get(&wrap).is_some_and(|r| r.machine == machine) {
                    let before = st.queue.len();
                    st.queue
                        .retain(|q| !(q.machine == machine && q.wrap.id == wrap));
                    if st.queue.len() != before {
                        inner.persist_queue(&st);
                    }
                    let state = if accepted {
                        DeliveryState::Received
                    } else {
                        DeliveryState::Rejected
                    };
                    inner.set_delivery(&mut st, wrap, machine, state);
                    inner.persist_delivery(&st);
                }
                None
            }
            Frame::Ack(ack) => {
                if verify_ack(&ack, &inner.guardian, &machine) {
                    let mut st = lock(&inner.state);
                    if st
                        .delivery
                        .get(&ack.wrap)
                        .is_some_and(|r| r.machine == machine)
                    {
                        inner.set_delivery(
                            &mut st,
                            ack.wrap,
                            machine,
                            DeliveryState::Acked(ack.result),
                        );
                        inner.persist_delivery(&st);
                    }
                }
                None
            }
            Frame::Ping(n) => Some(Frame::Pong(n)),
            Frame::Pong(_) | Frame::Unknown(_) => None,
            Frame::Hello { .. }
            | Frame::Challenge { .. }
            | Frame::Auth { .. }
            | Frame::Ready { .. } => return,
        };
        if let Some(f) = reply {
            if !send_frame(tx, &f).await {
                return;
            }
        }
    }
}

/// Store a ward wrap (once) and build the `OK` reply.
fn inbound_wrap(inner: &Inner, machine: PubKey, wrap: NostrEvent) -> Frame {
    let id = wrap.id;
    if validate_wrap_for(&wrap, &inner.guardian).is_err() {
        return Frame::Ok {
            wrap: id,
            accepted: false,
            reason: "invalid".into(),
        };
    }
    let mut st = lock(&inner.state);
    let now = now_unix();
    prune_seen(&mut st.seen, now);
    if st.seen.iter().any(|s| s.id == id) {
        return Frame::Ok {
            wrap: id,
            accepted: true,
            reason: "duplicate".into(),
        };
    }
    st.seen.push(SeenId { id, at: now });
    prune_seen(&mut st.seen, now);
    st.inbox.push(InboxEntry {
        id,
        machine,
        wrap: wrap.clone(),
        received_at: now,
    });
    if st.inbox.len() > INBOX_CAP {
        let excess = st.inbox.len() - INBOX_CAP;
        st.inbox.drain(..excess);
    }
    inner.persist_inbox(&st);
    inner.persist_seen(&st);
    inner.emit(GuardianEvent::Wrap { machine, wrap });
    Frame::Ok {
        wrap: id,
        accepted: true,
        reason: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_state_spellings_round_trip() {
        for s in [
            "queued",
            "sent",
            "received",
            "rejected",
            "applied",
            "unchanged",
            "superseded",
            "expired",
            "refused",
            "failed",
            "ignored",
        ] {
            assert_eq!(DeliveryState::parse(s).unwrap().as_str(), s);
        }
        assert_eq!(DeliveryState::parse("Sent"), None);
    }

    #[test]
    fn delivery_state_only_moves_forward() {
        use DeliveryState::*;
        assert!(Queued.advanced_by(Sent));
        assert!(Sent.advanced_by(Received));
        assert!(Received.advanced_by(Acked(AckResult::Applied)));
        assert!(!Received.advanced_by(Sent));
        assert!(!Acked(AckResult::Applied).advanced_by(Received));
        assert!(!Acked(AckResult::Applied).advanced_by(Acked(AckResult::Unchanged)));
        assert!(Acked(AckResult::Failed).advanced_by(Acked(AckResult::Applied)));
        assert!(!Sent.advanced_by(Sent));
        assert!(!Sent.advanced_by(Queued));
    }

    #[test]
    fn send_state_bridge_shape() {
        assert_eq!(
            serde_json::to_string(&SendState::Sent).unwrap(),
            r#"{"state":"sent"}"#
        );
        assert_eq!(
            serde_json::to_string(&SendState::Rejected("x".into())).unwrap(),
            r#"{"state":"rejected","reason":"x"}"#
        );
    }
}
