//! The ward role: connects out to the guardian carrier, proves its machine
//! key only after the guardian has proved the pinned guardian key, then
//! exchanges gift wraps.
//!
//! Inbound guardian wraps go to [`WardIngest`] (the broker's local ingest,
//! the same verification as a relay copy) one at a time, and each is
//! answered with a signed ACK. Outbound machine→guardian wraps go through a
//! persisted outbox ([`WardLink::sink`]) that every session drains.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::net::{SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use charter_primitives::{kinds, EventId, NostrEvent, PubKey};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::proto::{
    guardian_proof_verify, peer_address_allowed, public_key, sign_ack, validate_wrap_for,
    ward_proof_sign, Frame, PROTOCOL_VERSION,
};
use crate::session::{
    challenge, read_frame, send_frame, write_frame, writer, Closer, RateLimit, CONNECT_TIMEOUT,
    HANDSHAKE_TIMEOUT, IDLE_TIMEOUT, PING_INTERVAL,
};
use crate::store::{load, lock, now_unix, save_logged};
use crate::{AckResult, LocalOffer, LocalSink};

const MAX_CANDIDATES: usize = 8;
const MAX_SESSIONS: usize = 4;
const BACKOFF_START: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const SESSION_CHANNEL: usize = 64;
const OUTBOX_CAP: usize = 64;
const HOUR: u64 = 3600;

const OUTBOX_FILE: &str = "link-outbox.json";
const PEER_FILE: &str = "link-peer.json";

/// The broker's entry point for a wrap that arrived over the local link.
pub trait WardIngest: Send + Sync + 'static {
    fn ingest(&self, wrap: NostrEvent) -> Pin<Box<dyn Future<Output = AckResult> + Send>>;
}

/// Configuration for [`WardLink::spawn`].
pub struct WardLinkConfig {
    pub machine_sk: Zeroizing<[u8; 32]>,
    /// The pinned guardian: the only key this ward will authenticate to.
    pub guardian: PubKey,
    pub state_dir: PathBuf,
}

impl std::fmt::Debug for WardLinkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WardLinkConfig")
            .field("machine_sk", &"<redacted>")
            .field("guardian", &self.guardian)
            .field("state_dir", &self.state_dir)
            .finish()
    }
}

/// One live guardian session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WardSessionInfo {
    /// `ip:port` of the guardian carrier.
    pub addr: String,
    /// Unix seconds when the session became ready.
    pub since: u64,
}

/// What [`WardLink::snapshot`] reports. `connected` is true while any
/// authenticated session is up (platforms stop discovery then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WardLinkSnapshot {
    pub connected: bool,
    pub sessions: Vec<WardSessionInfo>,
    /// Addresses currently tried (discovered plus the remembered one).
    pub candidates: usize,
    /// The address that last completed a handshake, if any.
    pub remembered: Option<String>,
    /// Wraps waiting in the outbox.
    pub outbox: usize,
    /// The most recent connection or handshake failure, cleared on success.
    pub last_error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutEntry {
    #[serde(skip)]
    seq: u64,
    inner_kind: u16,
    key: Option<String>,
    offered_at: u64,
    wrap: NostrEvent,
}

#[derive(Serialize, Deserialize)]
struct OutboxFile {
    v: u8,
    entries: Vec<OutEntry>,
}

#[derive(Serialize, Deserialize)]
struct PeerFile {
    v: u8,
    guardian: PubKey,
    addr: String,
}

/// Retention class of an outbox entry: (class, ttl seconds).
fn retention(inner_kind: u16) -> (u8, u64) {
    match inner_kind {
        kinds::CHARTER_DEVICE_REQUEST => (0, 24 * HOUR),
        kinds::CHARTER_DEVICE_AUDIT => (1, 48 * HOUR),
        kinds::CHARTER_DEVICE_STATUS => (2, 48 * HOUR),
        _ => (3, 48 * HOUR),
    }
}

fn prune_outbox(outbox: &mut Vec<OutEntry>, now: u64) {
    outbox.retain(|e| now.saturating_sub(e.offered_at) < retention(e.inner_kind).1);
    for class in 0..=3u8 {
        let n = outbox
            .iter()
            .filter(|e| retention(e.inner_kind).0 == class)
            .count();
        if n > OUTBOX_CAP {
            let mut drop = n - OUTBOX_CAP;
            outbox.retain(|e| {
                if drop > 0 && retention(e.inner_kind).0 == class {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }
}

struct Backoff {
    next: Instant,
    delay: Duration,
}

struct WSession {
    addr: SocketAddrV4,
    since: u64,
    closer: Arc<Closer>,
}

struct State {
    closed: bool,
    discovered: Vec<SocketAddrV4>,
    remembered: Option<SocketAddrV4>,
    backoff: HashMap<SocketAddrV4, Backoff>,
    connecting: HashSet<SocketAddrV4>,
    sessions: HashMap<u64, WSession>,
    next_session: u64,
    outbox: Vec<OutEntry>,
    next_seq: u64,
    last_error: Option<String>,
}

impl State {
    fn candidates(&self) -> Vec<SocketAddrV4> {
        let mut out: Vec<SocketAddrV4> = Vec::new();
        for a in self.remembered.iter().chain(self.discovered.iter()) {
            if !out.contains(a) {
                out.push(*a);
            }
        }
        out.truncate(MAX_CANDIDATES);
        out
    }
}

struct Inner {
    sk: Zeroizing<[u8; 32]>,
    machine: PubKey,
    guardian: PubKey,
    dir: PathBuf,
    ingest: Arc<dyn WardIngest>,
    state: Mutex<State>,
    wake: Notify,
    outbox_changed: watch::Sender<u64>,
    shutdown: Closer,
}

/// The ward's local link client.
pub struct WardLink {
    inner: Arc<Inner>,
}

/// The ward outbox as a [`LocalSink`].
struct Outbox(Arc<Inner>);

impl LocalSink for Outbox {
    fn offer(&self, offer: LocalOffer) {
        self.0.offer(offer);
    }
}

/// Only addresses the guardian listener would itself accept a ward from;
/// loopback is allowed (it is this machine).
fn usable(a: &SocketAddrV4) -> bool {
    a.port() != 0 && peer_address_allowed((*a.ip()).into(), true)
}

impl WardLink {
    /// Load the outbox and remembered guardian address from `state_dir` and
    /// start connecting on `rt`.
    pub fn spawn(
        cfg: WardLinkConfig,
        ingest: Arc<dyn WardIngest>,
        rt: &tokio::runtime::Handle,
    ) -> io::Result<Arc<Self>> {
        let machine = public_key(&cfg.machine_sk)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid machine key"))?;
        std::fs::create_dir_all(&cfg.state_dir)?;
        let dir = cfg.state_dir.clone();
        let now = now_unix();

        let mut outbox = load::<OutboxFile>(&dir.join(OUTBOX_FILE))
            .map(|f| f.entries)
            .unwrap_or_default();
        outbox.sort_by_key(|e| e.offered_at);
        for (i, e) in outbox.iter_mut().enumerate() {
            e.seq = i as u64 + 1;
        }
        let next_seq = outbox.len() as u64 + 1;
        prune_outbox(&mut outbox, now);
        let remembered = load::<PeerFile>(&dir.join(PEER_FILE))
            .filter(|p| p.guardian == cfg.guardian)
            .and_then(|p| p.addr.parse::<SocketAddrV4>().ok())
            .filter(usable);

        let inner = Arc::new(Inner {
            sk: cfg.machine_sk,
            machine,
            guardian: cfg.guardian,
            dir,
            ingest,
            state: Mutex::new(State {
                closed: false,
                discovered: Vec::new(),
                remembered,
                backoff: HashMap::new(),
                connecting: HashSet::new(),
                sessions: HashMap::new(),
                next_session: 1,
                outbox,
                next_seq,
                last_error: None,
            }),
            wake: Notify::new(),
            outbox_changed: watch::channel(0).0,
            shutdown: Closer::default(),
        });
        rt.spawn(connector(inner.clone()));
        Ok(Arc::new(WardLink { inner }))
    }

    /// Replace the discovered guardian addresses (the remembered address
    /// stays a candidate) and retry at once.
    pub fn set_discovered(&self, addrs: Vec<SocketAddrV4>) {
        let mut st = lock(&self.inner.state);
        let mut d: Vec<SocketAddrV4> = Vec::new();
        for a in addrs.into_iter().filter(usable) {
            if !d.contains(&a) {
                d.push(a);
            }
        }
        st.discovered = d;
        st.backoff.clear();
        drop(st);
        self.inner.wake.notify_one();
    }

    /// A network change, screen-on or alarm: reset back-off and try now.
    pub fn kick(&self) {
        lock(&self.inner.state).backoff.clear();
        self.inner.wake.notify_one();
    }

    /// The ward outbox. Offers are recorded and handed to live sessions;
    /// this never waits on the network.
    pub fn sink(self: &Arc<Self>) -> Arc<dyn LocalSink> {
        Arc::new(Outbox(self.inner.clone()))
    }

    pub fn snapshot(&self) -> WardLinkSnapshot {
        let mut st = lock(&self.inner.state);
        prune_outbox(&mut st.outbox, now_unix());
        let mut sessions: Vec<WardSessionInfo> = st
            .sessions
            .values()
            .map(|s| WardSessionInfo {
                addr: s.addr.to_string(),
                since: s.since,
            })
            .collect();
        sessions.sort_by(|a, b| a.since.cmp(&b.since).then(a.addr.cmp(&b.addr)));
        WardLinkSnapshot {
            connected: !st.sessions.is_empty(),
            sessions,
            candidates: st.candidates().len(),
            remembered: st.remembered.map(|a| a.to_string()),
            outbox: st.outbox.len(),
            last_error: st.last_error.clone(),
        }
    }

    /// Stop connecting and close every session. Persisted state is left as
    /// of this call; a new link may start on the same directory at once.
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
        }
        self.inner.shutdown.close();
        self.inner.wake.notify_one();
    }
}

impl Drop for WardLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Inner {
    fn persist_outbox(&self, st: &State) {
        if !st.closed {
            save_logged(
                &self.dir.join(OUTBOX_FILE),
                &OutboxFile {
                    v: 1,
                    entries: st.outbox.clone(),
                },
            );
        }
    }

    fn offer(&self, offer: LocalOffer) {
        let seq = {
            let mut st = lock(&self.state);
            if st.closed {
                return;
            }
            let now = now_unix();
            prune_outbox(&mut st.outbox, now);
            if st.outbox.iter().any(|e| e.wrap.id == offer.wrap.id) {
                return;
            }
            if let Some(k) = &offer.key {
                st.outbox
                    .retain(|e| !(e.inner_kind == offer.inner_kind && e.key.as_ref() == Some(k)));
            }
            let seq = st.next_seq;
            st.next_seq += 1;
            st.outbox.push(OutEntry {
                seq,
                inner_kind: offer.inner_kind,
                key: offer.key,
                offered_at: now,
                wrap: offer.wrap,
            });
            prune_outbox(&mut st.outbox, now);
            self.persist_outbox(&st);
            seq
        };
        self.outbox_changed.send_replace(seq);
    }

    fn fail(&self, addr: SocketAddrV4, err: String) {
        let mut st = lock(&self.state);
        st.connecting.remove(&addr);
        let delay = match st.backoff.get(&addr) {
            Some(b) => (b.delay * 2).min(BACKOFF_MAX),
            None => BACKOFF_START,
        };
        st.backoff.insert(
            addr,
            Backoff {
                next: Instant::now() + delay,
                delay,
            },
        );
        st.last_error = Some(err);
        drop(st);
        self.wake.notify_one();
    }
}

/// Keeps trying every candidate that has no session, no attempt in flight
/// and no pending back-off, until shutdown.
async fn connector(inner: Arc<Inner>) {
    loop {
        if inner.shutdown.is_closed() {
            return;
        }
        let mut next_wake: Option<Instant> = None;
        let to_try: Vec<SocketAddrV4> = {
            let mut st = lock(&inner.state);
            let now = Instant::now();
            let mut v = Vec::new();
            // At the session limit, wait for one to close before dialling.
            let full = st.sessions.len() >= MAX_SESSIONS;
            for a in st.candidates().into_iter().filter(|_| !full) {
                if st.connecting.contains(&a) || st.sessions.values().any(|s| s.addr == a) {
                    continue;
                }
                match st.backoff.get(&a) {
                    Some(b) if b.next > now => {
                        next_wake = Some(next_wake.map_or(b.next, |w| w.min(b.next)));
                    }
                    _ => v.push(a),
                }
            }
            for a in &v {
                st.connecting.insert(*a);
            }
            v
        };
        for a in to_try {
            tokio::spawn(attempt(inner.clone(), a));
        }
        let sleep_until = next_wake.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600));
        tokio::select! {
            _ = inner.shutdown.closed() => return,
            _ = inner.wake.notified() => {}
            _ = tokio::time::sleep_until(sleep_until) => {}
        }
    }
}

async fn attempt(inner: Arc<Inner>, addr: SocketAddrV4) {
    let stream =
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(SocketAddr::V4(addr))).await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return inner.fail(addr, format!("connect {addr}: {e}")),
            Err(_) => return inner.fail(addr, format!("connect {addr}: timed out")),
        };
    let _ = stream.set_nodelay(true);
    let (mut rd, mut wr) = stream.into_split();
    let mut rate = RateLimit::new();
    let hs = tokio::select! {
        _ = inner.shutdown.closed() => Err("shutting down".to_owned()),
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&inner, &mut rd, &mut wr, &mut rate)) => {
            r.unwrap_or_else(|_| Err("handshake timed out".to_owned()))
        }
    };
    if let Err(e) = hs {
        return inner.fail(addr, format!("handshake with {addr}: {e}"));
    }

    let closer = Arc::new(Closer::default());
    let id = {
        let mut st = lock(&inner.state);
        st.connecting.remove(&addr);
        if st.closed
            || st.sessions.len() >= MAX_SESSIONS
            || st.sessions.values().any(|s| s.addr == addr)
        {
            return;
        }
        let id = st.next_session;
        st.next_session += 1;
        st.sessions.insert(
            id,
            WSession {
                addr,
                since: now_unix(),
                closer: closer.clone(),
            },
        );
        st.backoff.remove(&addr);
        st.last_error = None;
        if st.remembered != Some(addr) {
            st.remembered = Some(addr);
            save_logged(
                &inner.dir.join(PEER_FILE),
                &PeerFile {
                    v: 1,
                    guardian: inner.guardian,
                    addr: addr.to_string(),
                },
            );
        }
        id
    };
    eprintln!("charter-link: guardian session up at {addr}");

    let (tx, rx) = mpsc::channel::<Vec<u8>>(SESSION_CHANNEL);
    let writer_task = tokio::spawn(writer(wr, rx, closer.clone()));
    let pump_task = tokio::spawn(pump(inner.clone(), tx.clone(), closer.clone()));
    let ping_task = tokio::spawn(pinger(tx.clone()));

    tokio::select! {
        _ = inner.shutdown.closed() => {}
        _ = closer.closed() => {}
        _ = session(&inner, &mut rd, &tx, &mut rate) => {}
    }

    closer.close();
    writer_task.abort();
    pump_task.abort();
    ping_task.abort();
    {
        let mut st = lock(&inner.state);
        st.sessions.remove(&id);
        // A session that ran normally is retried after the initial back-off.
        st.backoff.insert(
            addr,
            Backoff {
                next: Instant::now() + BACKOFF_START,
                delay: BACKOFF_START,
            },
        );
    }
    eprintln!("charter-link: guardian session closed at {addr}");
    inner.wake.notify_one();
}

/// HELLO → CHALLENGE (verify the pinned guardian) → AUTH → READY.
async fn handshake(
    inner: &Inner,
    rd: &mut tokio::net::tcp::OwnedReadHalf,
    wr: &mut tokio::net::tcp::OwnedWriteHalf,
    rate: &mut RateLimit,
) -> Result<(), String> {
    let cw = challenge().map_err(|e| format!("no entropy: {e}"))?;
    write_frame(
        wr,
        &Frame::Hello {
            v: PROTOCOL_VERSION as u64,
            challenge: cw,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let (guardian, cg, sig) = match read_frame(rd).await.map_err(|e| e.to_string())? {
        Frame::Challenge {
            v,
            guardian,
            challenge,
            sig,
        } if v == PROTOCOL_VERSION as u64 => (guardian, challenge, sig),
        _ => return Err("unexpected frame".into()),
    };
    rate.hit();
    if guardian != inner.guardian {
        return Err("not the pinned guardian".into());
    }
    if !guardian_proof_verify(&cw, &cg, &guardian, &sig) {
        return Err("guardian proof did not verify".into());
    }
    let sw = ward_proof_sign(&inner.sk, &cw, &cg, &guardian).ok_or("cannot sign")?;
    write_frame(
        wr,
        &Frame::Auth {
            machine: inner.machine,
            sig: sw,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    // The guardian closes without explanation when it does not admit us.
    match read_frame(rd).await {
        Ok(Frame::Ready { v }) if v == PROTOCOL_VERSION as u64 => {}
        _ => return Err("not admitted".into()),
    }
    rate.hit();
    Ok(())
}

/// Send every outbox entry oldest first, then each new offer as it lands.
async fn pump(inner: Arc<Inner>, tx: mpsc::Sender<Vec<u8>>, closer: Arc<Closer>) {
    let mut changed = inner.outbox_changed.subscribe();
    let mut cursor = 0u64;
    loop {
        let batch: Vec<(u64, NostrEvent)> = {
            let mut st = lock(&inner.state);
            prune_outbox(&mut st.outbox, now_unix());
            let mut v: Vec<(u64, NostrEvent)> = st
                .outbox
                .iter()
                .filter(|e| e.seq > cursor)
                .map(|e| (e.seq, e.wrap.clone()))
                .collect();
            v.sort_by_key(|(s, _)| *s);
            v
        };
        for (seq, wrap) in batch {
            if !send_frame(&tx, &Frame::Event(wrap)).await {
                closer.close();
                return;
            }
            cursor = seq;
        }
        tokio::select! {
            _ = closer.closed() => return,
            r = changed.changed() => if r.is_err() { return },
        }
    }
}

async fn pinger(tx: mpsc::Sender<Vec<u8>>) {
    let mut n = 0u64;
    loop {
        tokio::time::sleep(PING_INTERVAL).await;
        n += 1;
        if !send_frame(&tx, &Frame::Ping(n)).await {
            return;
        }
    }
}

/// The authenticated session's inbound loop: one frame at a time, so a
/// slow ingest pauses reading.
async fn session(
    inner: &Inner,
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
            eprintln!("charter-link: guardian session over its frame rate; closing");
            return;
        }
        match frame {
            Frame::Event(wrap) => {
                let id = wrap.id;
                if validate_wrap_for(&wrap, &inner.machine).is_err() {
                    if !send_frame(
                        tx,
                        &Frame::Ok {
                            wrap: id,
                            accepted: false,
                            reason: "invalid".into(),
                        },
                    )
                    .await
                    {
                        return;
                    }
                    continue;
                }
                if !send_frame(
                    tx,
                    &Frame::Ok {
                        wrap: id,
                        accepted: true,
                        reason: String::new(),
                    },
                )
                .await
                {
                    return;
                }
                let result = inner.ingest.ingest(wrap).await;
                let ack = sign_ack(&inner.sk, &inner.guardian, &id, result, now_unix());
                if !send_frame(tx, &Frame::Ack(ack)).await {
                    return;
                }
            }
            Frame::Ok { wrap, .. } => remove_from_outbox(inner, &wrap),
            Frame::Ping(n) => {
                if !send_frame(tx, &Frame::Pong(n)).await {
                    return;
                }
            }
            Frame::Pong(_) | Frame::Ack(_) | Frame::Unknown(_) => {}
            Frame::Hello { .. }
            | Frame::Challenge { .. }
            | Frame::Auth { .. }
            | Frame::Ready { .. } => return,
        }
    }
}

/// The guardian has stored (or refused) a wrap: it leaves the outbox.
fn remove_from_outbox(inner: &Inner, wrap: &EventId) {
    let mut st = lock(&inner.state);
    let before = st.outbox.len();
    st.outbox.retain(|e| e.wrap.id != *wrap);
    if st.outbox.len() != before {
        inner.persist_outbox(&st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_classes() {
        assert_eq!(retention(kinds::CHARTER_DEVICE_REQUEST), (0, 24 * HOUR));
        assert_eq!(retention(kinds::CHARTER_DEVICE_AUDIT), (1, 48 * HOUR));
        assert_eq!(retention(kinds::CHARTER_DEVICE_STATUS), (2, 48 * HOUR));
        assert_eq!(retention(kinds::CHARTER_DEVICE_USAGE_SYNC), (3, 48 * HOUR));
    }

    #[test]
    fn usable_addresses() {
        assert!(usable(&"192.168.1.5:47615".parse().unwrap()));
        assert!(usable(&"127.0.0.1:47615".parse().unwrap()));
        assert!(!usable(&"192.168.1.5:0".parse().unwrap()));
        assert!(!usable(&"8.8.8.8:47615".parse().unwrap()));
    }
}
