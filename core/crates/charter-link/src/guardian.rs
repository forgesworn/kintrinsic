//! The guardian role: a TCP listener on the carrier that wards connect to.
//!
//! The listener proves the guardian key to anyone who says HELLO (the key is
//! already public on relays), admits only machine keys in the allowed set,
//! and then exchanges gift wraps. It never interprets a wrap: inbound wraps
//! go to a persisted inbox for the console's normal ingest, outbound wraps
//! are written to a live session or queued until the ward connects.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener as StdListener};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use charter_primitives::{EventId, NostrEvent, PubKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
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
use crate::store::{load, lock, now_unix, Persist};
use crate::AckResult;

const MAX_UNAUTHENTICATED: usize = 8;
/// Pre-handshake connections from any one source address.
const MAX_UNAUTHENTICATED_PER_IP: usize = 2;
const MAX_SESSIONS: usize = 16;
// Matches the ward's own session cap, so a carrier reachable at several
// addresses never evicts and re-admits the same ward in a loop.
const MAX_SESSIONS_PER_MACHINE: usize = 4;
const QUEUE_PER_MACHINE: usize = 64;
const QUEUE_TTL: u64 = 48 * 3600;
const DELIVERY_CAP: usize = 512;
const DELIVERY_TTL: u64 = 7 * 24 * 3600;
const INBOX_CAP: usize = 256;
/// Total (approximate encoded) size of the inbox.
const INBOX_BYTES: usize = 8 * 1024 * 1024;
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

    /// Whether `next` should replace `self`. States only move forward
    /// (`received` and `rejected` are both final short of an ACK), a later
    /// ACK replaces only a `failed` one, and repeats are no-ops.
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
    /// The authenticated machine key of the session it arrived on. This is
    /// not necessarily the seal author (any paired ward can relay a wrap it
    /// saw on relays); the console's seal check decides that.
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
    /// The carrier's own address this session was accepted on.
    local: IpAddr,
    since: u64,
    started: Instant,
    tx: mpsc::Sender<Vec<u8>>,
    closer: Arc<Closer>,
    /// Queued wraps already written on this session (not re-sent on it).
    sent: HashSet<EventId>,
    /// Wakes this session's flusher when a wrap for its machine is queued.
    wake: Arc<Notify>,
}

struct State {
    closed: bool,
    /// False while the carrier is off any home network: connections are
    /// refused, sessions closed; queue and inbox are kept.
    accepting: bool,
    /// The carrier's own IPv4 addresses a ward may connect to (its Wi-Fi and
    /// Ethernet interfaces); `None` admits any. See
    /// [`GuardianLink::set_accept_addresses`].
    accept_addrs: Option<HashSet<Ipv4Addr>>,
    allowed: HashSet<PubKey>,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    /// Every guardian wrap not yet answered by its ward's `OK`, including
    /// those already written to a live session.
    queue: Vec<Queued>,
    delivery: BTreeMap<EventId, DeliveryRecord>,
    inbox: Vec<InboxEntry>,
    /// Approximate encoded size of `inbox`, see [`wrap_bytes`].
    inbox_bytes: usize,
    seen: Vec<SeenId>,
}

/// Which persisted files a change touched.
#[derive(Default, Clone, Copy)]
struct Dirty {
    queue: bool,
    delivery: bool,
    inbox: bool,
    seen: bool,
}

impl Dirty {
    fn or(self, o: Dirty) -> Dirty {
        Dirty {
            queue: self.queue || o.queue,
            delivery: self.delivery || o.delivery,
            inbox: self.inbox || o.inbox,
            seen: self.seen || o.seen,
        }
    }

    fn any(&self) -> bool {
        self.queue || self.delivery || self.inbox || self.seen
    }
}

/// Pending events, with the wrap bytes they hold.
#[derive(Default)]
struct Events {
    q: VecDeque<GuardianEvent>,
    wrap_bytes: usize,
}

struct Files {
    queue: Arc<Persist<QueueFile>>,
    delivery: Arc<Persist<DeliveryFile>>,
    inbox: Arc<Persist<InboxFile>>,
    seen: Arc<Persist<SeenFile>>,
}

struct Inner {
    sk: Zeroizing<[u8; 32]>,
    guardian: PubKey,
    allow_loopback: bool,
    state: Mutex<State>,
    files: Files,
    events: Mutex<Events>,
    events_cv: Condvar,
    unauthenticated: Mutex<Unauth>,
    shutdown: Arc<Closer>,
}

/// Connections that have not finished the handshake.
#[derive(Default)]
struct Unauth {
    total: usize,
    per_ip: HashMap<IpAddr, usize>,
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

        let queue = load::<QueueFile>(&dir.join(OUTBOUND_FILE));
        let delivery = load::<DeliveryFile>(&dir.join(DELIVERY_FILE));
        let inbox = load::<InboxFile>(&dir.join(INBOX_FILE));
        let seen = load::<SeenFile>(&dir.join(SEEN_FILE));
        let files = Files {
            queue: Persist::new(dir.join(OUTBOUND_FILE), queue.writable),
            delivery: Persist::new(dir.join(DELIVERY_FILE), delivery.writable),
            inbox: Persist::new(dir.join(INBOX_FILE), inbox.writable),
            seen: Persist::new(dir.join(SEEN_FILE), seen.writable),
        };
        let state = State {
            closed: false,
            accepting: true,
            accept_addrs: None,
            allowed: allowed.into_iter().collect(),
            sessions: HashMap::new(),
            next_session: 1,
            queue: queue.value.map(|f| f.queue).unwrap_or_default(),
            delivery: delivery
                .value
                .map(|f| f.records.into_iter().map(|e| (e.wrap, e.record)).collect())
                .unwrap_or_default(),
            inbox: inbox.value.map(|f| f.entries).unwrap_or_default(),
            inbox_bytes: 0,
            seen: seen.value.map(|f| f.seen).unwrap_or_default(),
        };

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
            allow_loopback: cfg.allow_loopback,
            state: Mutex::new(state),
            files,
            events: Mutex::new(Events::default()),
            events_cv: Condvar::new(),
            unauthenticated: Mutex::new(Unauth::default()),
            shutdown: Arc::new(Closer::default()),
        });
        let d = {
            let mut st = lock(&inner.state);
            let mut d = inner.prune_queue(&mut st, now);
            let n = st.delivery.len();
            prune_delivery(&mut st.delivery, now);
            d.delivery |= st.delivery.len() != n;
            let n = st.seen.len();
            prune_seen(&mut st.seen, now);
            d.seen |= st.seen.len() != n;
            st.inbox_bytes = st.inbox.iter().map(|e| wrap_bytes(&e.wrap)).sum();
            d.inbox |= trim_inbox(&mut st);
            inner.stage(&st, d);
            d
        };
        inner.flush(d);
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

    /// Accept ward connections only on these local IPv4 addresses (the
    /// carrier's Wi-Fi and Ethernet interfaces), or on any (`None`, the
    /// default). The listener is bound to every interface, so without this a
    /// phone on Wi-Fi with mobile data up would also take connections that
    /// arrive over mobile data. A connection whose local address is not in the
    /// set closes at once, and so does every session accepted on an address
    /// that has left it. An empty set admits nothing.
    pub fn set_accept_addresses(&self, addrs: Option<Vec<Ipv4Addr>>) {
        let mut st = lock(&self.inner.state);
        st.accept_addrs = addrs.map(|v| v.into_iter().collect());
        let gone: Vec<u64> = st
            .sessions
            .iter()
            .filter(|(_, s)| !local_allowed(&st, s.local))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.inner.drop_session(&mut st, id);
        }
    }

    /// Send a wrap to a ward: written at once to a live session, otherwise
    /// queued (persisted) until the ward connects. Either way the wrap is
    /// held until the ward's `OK`, so a session that dies before the ward
    /// reads it sends it again at the next READY.
    pub fn send(&self, recipient: &PubKey, wrap: NostrEvent) -> SendState {
        if let Err(e) = validate_wrap_for(&wrap, recipient) {
            return SendState::Rejected(e.to_string());
        }
        let inner = &self.inner;
        let id = wrap.id;
        let (result, d) = {
            let mut st = lock(&inner.state);
            if st.closed {
                return SendState::Rejected("link stopped".into());
            }
            if !st.allowed.contains(recipient) {
                return SendState::Rejected("recipient is not a paired device".into());
            }
            let now = now_unix();
            let mut d = Dirty::default();
            if !st
                .queue
                .iter()
                .any(|q| q.machine == *recipient && q.wrap.id == id)
            {
                st.queue.push(Queued {
                    machine: *recipient,
                    wrap: wrap.clone(),
                    queued_at: now,
                });
                d.queue = true;
            }
            d = d.or(inner.prune_queue(&mut st, now));
            let newest = st
                .sessions
                .iter()
                .filter(|(_, s)| s.machine == *recipient && !s.closer.is_closed())
                .max_by_key(|(_, s)| s.started)
                .map(|(sid, _)| *sid);
            let sent = match newest.and_then(|sid| st.sessions.get_mut(&sid)) {
                Some(s) if s.sent.contains(&id) => true,
                Some(s) => {
                    let frame = crate::proto::encode_frame(&Frame::Event(wrap));
                    if s.tx.try_send(frame).is_ok() {
                        s.sent.insert(id);
                        true
                    } else {
                        // The session's channel is full: its flusher sends
                        // it as soon as there is room.
                        s.wake.notify_one();
                        false
                    }
                }
                None => false,
            };
            let state = if sent {
                DeliveryState::Sent
            } else {
                DeliveryState::Queued
            };
            d.delivery |= inner.set_delivery(&mut st, id, *recipient, state);
            inner.stage(&st, d);
            let result = if sent {
                SendState::Sent
            } else {
                SendState::Queued
            };
            (result, d)
        };
        inner.flush(d);
        result
    }

    /// Wait up to `timeout` for events and return all that are pending.
    /// Blocking: meant for a dedicated (non-async) thread. Returns early,
    /// possibly empty, after [`GuardianLink::shutdown`].
    pub fn next_events(&self, timeout: Duration) -> Vec<GuardianEvent> {
        let deadline = std::time::Instant::now() + timeout;
        let mut ev = lock(&self.inner.events);
        while ev.q.is_empty() && !self.inner.shutdown.is_closed() {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            ev = match self.inner.events_cv.wait_timeout(ev, deadline - now) {
                Ok((g, _)) => g,
                Err(e) => e.into_inner().0,
            };
        }
        ev.wrap_bytes = 0;
        ev.q.drain(..).collect()
    }

    /// Ward wraps held for the console, oldest first.
    pub fn inbox(&self) -> Vec<InboxEntry> {
        lock(&self.inner.state).inbox.clone()
    }

    /// Drop inbox entries the console has ingested. Their ids stay in the
    /// seen set, so a later duplicate is not stored again.
    pub fn consume(&self, ids: &[EventId]) {
        let d = {
            let mut st = lock(&self.inner.state);
            let before = st.inbox.len();
            st.inbox.retain(|e| !ids.contains(&e.id));
            if st.inbox.len() == before {
                return;
            }
            st.inbox_bytes = st.inbox.iter().map(|e| wrap_bytes(&e.wrap)).sum();
            let d = Dirty {
                inbox: true,
                ..Dirty::default()
            };
            self.inner.stage(&st, d);
            d
        };
        self.inner.flush(d);
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
    /// as of this call (pending writes land before it returns); a new link
    /// may start on the same directory at once.
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
        let f = &self.inner.files;
        f.queue.close();
        f.delivery.close();
        f.inbox.close();
        f.seen.close();
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

/// Drop expired entries and the oldest beyond each machine's cap; returns
/// what was dropped.
fn prune_queue(q: &mut Vec<Queued>, now: u64) -> Vec<(PubKey, EventId)> {
    let mut dropped = Vec::new();
    q.retain(|e| {
        let keep = now.saturating_sub(e.queued_at) < QUEUE_TTL;
        if !keep {
            dropped.push((e.machine, e.wrap.id));
        }
        keep
    });
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
                    dropped.push((e.machine, e.wrap.id));
                    false
                } else {
                    true
                }
            });
        }
    }
    dropped
}

/// A record the console no longer waits on: evicted first.
fn is_terminal(s: DeliveryState) -> bool {
    matches!(s, DeliveryState::Rejected | DeliveryState::Acked(_))
}

fn prune_delivery(d: &mut BTreeMap<EventId, DeliveryRecord>, now: u64) {
    d.retain(|_, r| now.saturating_sub(r.at) < DELIVERY_TTL);
    while d.len() > DELIVERY_CAP {
        let oldest = d
            .iter()
            .min_by_key(|(_, r)| (!is_terminal(r.state), r.at))
            .map(|(id, _)| *id);
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

/// Approximate encoded size of a wrap, for the byte bounds.
fn wrap_bytes(w: &NostrEvent) -> usize {
    // id, pubkey, sig, kind, created_at and JSON punctuation.
    const FIXED: usize = 320;
    FIXED + w.content.len() + w.tags.iter().flatten().map(|s| s.len() + 4).sum::<usize>()
}

/// Enforce the inbox bounds, by count and by bytes, each time evicting the
/// oldest entry of the machine holding the most, so a ward that floods the
/// inbox pushes out its own entries before another ward's. `true` if
/// anything was dropped.
fn trim_inbox(st: &mut State) -> bool {
    let mut changed = false;
    while st.inbox.len() > INBOX_CAP || st.inbox_bytes > INBOX_BYTES {
        // (machine, count, bytes) in order of each machine's oldest entry.
        let mut per: Vec<(PubKey, usize, usize)> = Vec::new();
        for e in &st.inbox {
            let b = wrap_bytes(&e.wrap);
            match per.iter_mut().find(|(m, _, _)| *m == e.machine) {
                Some(p) => {
                    p.1 += 1;
                    p.2 += b;
                }
                None => per.push((e.machine, 1, b)),
            }
        }
        let most = |key: fn(&(PubKey, usize, usize)) -> usize| {
            let mut best: Option<&(PubKey, usize, usize)> = None;
            for p in &per {
                if best.is_none_or(|b| key(p) > key(b)) {
                    best = Some(p);
                }
            }
            best.map(|p| p.0)
        };
        let victim = if st.inbox.len() > INBOX_CAP {
            most(|p| p.1)
        } else if st.inbox_bytes > INBOX_BYTES {
            most(|p| p.2)
        } else {
            None
        };
        let Some(m) = victim else { break };
        let Some(pos) = st.inbox.iter().position(|e| e.machine == m) else {
            break;
        };
        let e = st.inbox.remove(pos);
        st.inbox_bytes = st.inbox_bytes.saturating_sub(wrap_bytes(&e.wrap));
        changed = true;
    }
    changed
}

impl Inner {
    fn emit(&self, ev: GuardianEvent) {
        let mut evs = lock(&self.events);
        if let GuardianEvent::Wrap { wrap, .. } = &ev {
            evs.wrap_bytes += wrap_bytes(wrap);
        }
        evs.q.push_back(ev);
        // Bounded by count, and by the wraps they carry (no more than the
        // inbox itself holds): the oldest wrap events go first, since the
        // console can always read the inbox.
        while evs.q.len() > EVENTS_CAP || evs.wrap_bytes > INBOX_BYTES {
            let pos = if evs.wrap_bytes > INBOX_BYTES {
                evs.q
                    .iter()
                    .position(|e| matches!(e, GuardianEvent::Wrap { .. }))
                    .unwrap_or(0)
            } else {
                0
            };
            match evs.q.remove(pos) {
                Some(GuardianEvent::Wrap { wrap, .. }) => {
                    evs.wrap_bytes = evs.wrap_bytes.saturating_sub(wrap_bytes(&wrap));
                }
                Some(_) => {}
                None => break,
            }
        }
        self.events_cv.notify_all();
    }

    /// Move a record forward; `true` if it changed (delivery file dirty).
    fn set_delivery(
        &self,
        st: &mut State,
        wrap: EventId,
        machine: PubKey,
        state: DeliveryState,
    ) -> bool {
        let changed = match st.delivery.get(&wrap) {
            Some(r) if r.machine == machine => r.state.advanced_by(state),
            Some(_) => false,
            None => true,
        };
        if !changed {
            return false;
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
        true
    }

    /// Prune the queue; a dropped wrap that was still waiting is recorded
    /// as `rejected`, so the console stops waiting on the local route.
    fn prune_queue(&self, st: &mut State, now: u64) -> Dirty {
        let dropped = prune_queue(&mut st.queue, now);
        let mut d = Dirty::default();
        if dropped.is_empty() {
            return d;
        }
        d.queue = true;
        for (m, id) in dropped {
            let waiting = st.delivery.get(&id).is_some_and(|r| {
                r.machine == m && matches!(r.state, DeliveryState::Queued | DeliveryState::Sent)
            });
            if waiting {
                d.delivery |= self.set_delivery(st, id, m, DeliveryState::Rejected);
            }
        }
        d
    }

    /// Snapshot the dirty stores. Call with the state lock held, then
    /// [`Inner::flush`] or [`Inner::flush_async`] after releasing it.
    fn stage(&self, st: &State, d: Dirty) {
        if st.closed {
            return;
        }
        if d.queue {
            self.files.queue.stage(QueueFile {
                v: 1,
                queue: st.queue.clone(),
            });
        }
        if d.delivery {
            let records = st
                .delivery
                .iter()
                .map(|(id, r)| DeliveryEntry {
                    wrap: *id,
                    record: r.clone(),
                })
                .collect();
            self.files.delivery.stage(DeliveryFile { v: 1, records });
        }
        if d.inbox {
            self.files.inbox.stage(InboxFile {
                v: 1,
                entries: st.inbox.clone(),
            });
        }
        if d.seen {
            self.files.seen.stage(SeenFile {
                v: 1,
                seen: st.seen.clone(),
            });
        }
    }

    /// Write the staged snapshots of the dirty stores. Blocking.
    fn flush(&self, d: Dirty) {
        if d.queue {
            self.files.queue.flush();
        }
        if d.delivery {
            self.files.delivery.flush();
        }
        if d.inbox {
            self.files.inbox.flush();
        }
        if d.seen {
            self.files.seen.flush();
        }
    }

    /// [`Inner::flush`] on the blocking pool, from a session task.
    async fn flush_async(self: &Arc<Self>, d: Dirty) {
        if d.any() {
            let me = self.clone();
            let _ = tokio::task::spawn_blocking(move || me.flush(d)).await;
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

/// Whether a connection that arrived on the carrier's address `local` may be
/// served (see [`GuardianLink::set_accept_addresses`]).
fn local_allowed(st: &State, local: IpAddr) -> bool {
    let Some(set) = &st.accept_addrs else {
        return true;
    };
    let v4 = match local {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4,
            None => return false,
        },
    };
    set.contains(&v4)
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
        let Ok(local) = stream.local_addr().map(|a| a.ip()) else {
            drop(stream);
            continue;
        };
        let admitted = {
            let st = lock(&inner.state);
            st.accepting && local_allowed(&st, local)
        };
        if !peer_address_allowed(peer.ip(), inner.allow_loopback) || !admitted {
            drop(stream);
            continue;
        }
        let Some(slot) = Unauthenticated::take(&inner, peer.ip()) else {
            drop(stream);
            continue;
        };
        tokio::spawn(connection(inner.clone(), stream, peer, local, slot));
    }
}

/// One pre-handshake slot, released when the handshake ends either way.
struct Unauthenticated {
    inner: Arc<Inner>,
    ip: IpAddr,
}

impl Unauthenticated {
    /// A slot for `ip`, if neither the global nor its per-source cap is full.
    fn take(inner: &Arc<Inner>, ip: IpAddr) -> Option<Self> {
        let mut u = lock(&inner.unauthenticated);
        let mine = u.per_ip.get(&ip).copied().unwrap_or(0);
        if u.total >= MAX_UNAUTHENTICATED || mine >= MAX_UNAUTHENTICATED_PER_IP {
            return None;
        }
        u.total += 1;
        u.per_ip.insert(ip, mine + 1);
        Some(Unauthenticated {
            inner: inner.clone(),
            ip,
        })
    }
}

impl Drop for Unauthenticated {
    fn drop(&mut self) {
        let mut u = lock(&self.inner.unauthenticated);
        u.total = u.total.saturating_sub(1);
        if let Some(n) = u.per_ip.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                u.per_ip.remove(&self.ip);
            }
        }
    }
}

async fn connection(
    inner: Arc<Inner>,
    stream: TcpStream,
    peer: SocketAddr,
    local: IpAddr,
    slot: Unauthenticated,
) {
    let _ = stream.set_nodelay(true);
    let (mut rd, mut wr) = stream.into_split();
    let mut rate = RateLimit::new();
    let machine = {
        let _slot = slot;
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
    let wake = Arc::new(Notify::new());
    let (tx, rx) = mpsc::channel::<Vec<u8>>(SESSION_CHANNEL);
    let id = {
        let mut st = lock(&inner.state);
        if st.closed
            || !st.accepting
            || !local_allowed(&st, local)
            || !st.allowed.contains(&machine)
        {
            return;
        }
        // A fifth session from the same machine closes its oldest, before
        // the global cap is applied (stale half-open sessions after a
        // network change must not lock the machine out).
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
        if st.sessions.len() >= MAX_SESSIONS {
            if mine.is_empty() {
                return;
            }
            // Still full: this machine's own oldest makes room.
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
                local,
                since: now_unix(),
                started: Instant::now(),
                tx: tx.clone(),
                closer: closer.clone(),
                sent: HashSet::new(),
                wake: wake.clone(),
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
        _ = flusher(&inner, id, machine, &tx, &wake) => {}
        _ = session(&inner, id, machine, &mut rd, &tx, &mut rate) => {}
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

/// Write every queued wrap for `machine` not yet written on this session,
/// oldest first, then again each time `wake` fires. Entries stay queued
/// until the ward's `OK`, so a session that drops mid-flight loses nothing.
/// Returns only when the session can no longer be written to.
async fn flusher(
    inner: &Arc<Inner>,
    sid: u64,
    machine: PubKey,
    tx: &mpsc::Sender<Vec<u8>>,
    wake: &Notify,
) {
    loop {
        let (batch, d) = {
            let mut st = lock(&inner.state);
            let mut d = inner.prune_queue(&mut st, now_unix());
            let queued: Vec<&NostrEvent> = st
                .queue
                .iter()
                .filter(|q| q.machine == machine)
                .map(|q| &q.wrap)
                .collect();
            let queued_ids: HashSet<EventId> = queued.iter().map(|w| w.id).collect();
            let fresh: Vec<NostrEvent> = match st.sessions.get(&sid) {
                Some(s) => queued
                    .into_iter()
                    .filter(|w| !s.sent.contains(&w.id))
                    .cloned()
                    .collect(),
                None => return,
            };
            if let Some(s) = st.sessions.get_mut(&sid) {
                s.sent.retain(|id| queued_ids.contains(id));
                s.sent.extend(fresh.iter().map(|w| w.id));
            }
            // Marked sent before the frame is queued, so the ward's OK can
            // never arrive ahead of the record it answers.
            for w in &fresh {
                d.delivery |= inner.set_delivery(&mut st, w.id, machine, DeliveryState::Sent);
            }
            inner.stage(&st, d);
            (fresh, d)
        };
        inner.flush_async(d).await;
        for wrap in batch {
            if !send_frame(tx, &Frame::Event(wrap)).await {
                return;
            }
        }
        wake.notified().await;
    }
}

/// The authenticated session's inbound loop.
async fn session(
    inner: &Arc<Inner>,
    sid: u64,
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
            Frame::Event(wrap) => {
                let (reply, d) = inbound_wrap(inner, machine, wrap);
                // Stored on disk before the ward drops it from its outbox.
                inner.flush_async(d).await;
                Some(reply)
            }
            Frame::Ok { wrap, accepted, .. } => {
                let d = {
                    let mut st = lock(&inner.state);
                    let mut d = Dirty::default();
                    // Only for a wrap actually written to this machine.
                    let on_session = st
                        .sessions
                        .get(&sid)
                        .is_some_and(|s| s.sent.contains(&wrap));
                    let record = st.delivery.get(&wrap).filter(|r| r.machine == machine);
                    let was_sent = record.is_some_and(|r| r.state != DeliveryState::Queued);
                    let has_record = record.is_some();
                    if on_session || was_sent {
                        let before = st.queue.len();
                        st.queue
                            .retain(|q| !(q.machine == machine && q.wrap.id == wrap));
                        d.queue = st.queue.len() != before;
                        if has_record {
                            let state = if accepted {
                                DeliveryState::Received
                            } else {
                                DeliveryState::Rejected
                            };
                            d.delivery = inner.set_delivery(&mut st, wrap, machine, state);
                        }
                        inner.stage(&st, d);
                    }
                    d
                };
                inner.flush_async(d).await;
                None
            }
            Frame::Ack(ack) => {
                if verify_ack(&ack, &inner.guardian, &machine) {
                    let d = {
                        let mut st = lock(&inner.state);
                        let mut d = Dirty::default();
                        let was_sent = st.delivery.get(&ack.wrap).is_some_and(|r| {
                            r.machine == machine && r.state != DeliveryState::Queued
                        });
                        if was_sent {
                            d.delivery = inner.set_delivery(
                                &mut st,
                                ack.wrap,
                                machine,
                                DeliveryState::Acked(ack.result),
                            );
                            inner.stage(&st, d);
                        }
                        d
                    };
                    inner.flush_async(d).await;
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

/// Store a ward wrap (once) and build the `OK` reply. The caller writes the
/// dirty stores before sending it.
fn inbound_wrap(inner: &Inner, machine: PubKey, wrap: NostrEvent) -> (Frame, Dirty) {
    let id = wrap.id;
    if validate_wrap_for(&wrap, &inner.guardian).is_err() {
        let reply = Frame::Ok {
            wrap: id,
            accepted: false,
            reason: "invalid".into(),
        };
        return (reply, Dirty::default());
    }
    let mut st = lock(&inner.state);
    let now = now_unix();
    prune_seen(&mut st.seen, now);
    if st.seen.iter().any(|s| s.id == id) {
        let reply = Frame::Ok {
            wrap: id,
            accepted: true,
            reason: "duplicate".into(),
        };
        return (reply, Dirty::default());
    }
    st.seen.push(SeenId { id, at: now });
    prune_seen(&mut st.seen, now);
    st.inbox_bytes += wrap_bytes(&wrap);
    st.inbox.push(InboxEntry {
        id,
        machine,
        wrap: wrap.clone(),
        received_at: now,
    });
    trim_inbox(&mut st);
    let d = Dirty {
        inbox: true,
        seen: true,
        ..Dirty::default()
    };
    inner.stage(&st, d);
    inner.emit(GuardianEvent::Wrap { machine, wrap });
    let reply = Frame::Ok {
        wrap: id,
        accepted: true,
        reason: String::new(),
    };
    (reply, d)
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
        assert!(Sent.advanced_by(Rejected));
        assert!(!Received.advanced_by(Rejected));
        assert!(!Rejected.advanced_by(Received));
        assert!(Rejected.advanced_by(Acked(AckResult::Applied)));
    }

    #[test]
    fn delivery_pruning_evicts_finished_records_before_waiting_ones() {
        let m = PubKey::from_bytes([7; 32]);
        let now = 10_000;
        let mut d = BTreeMap::new();
        // The oldest record is still waiting; the rest are finished.
        d.insert(
            EventId::from_bytes([0; 32]),
            DeliveryRecord {
                state: DeliveryState::Sent,
                at: now - 100,
                machine: m,
            },
        );
        for i in 1..=DELIVERY_CAP as u32 {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&i.to_be_bytes());
            d.insert(
                EventId::from_bytes(id),
                DeliveryRecord {
                    state: DeliveryState::Acked(AckResult::Applied),
                    at: now - 50 + (i as u64 % 7),
                    machine: m,
                },
            );
        }
        prune_delivery(&mut d, now);
        assert_eq!(d.len(), DELIVERY_CAP);
        assert!(d.contains_key(&EventId::from_bytes([0; 32])));
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
