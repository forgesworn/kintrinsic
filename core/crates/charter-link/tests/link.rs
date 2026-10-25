//! Local link integration tests over real loopback sockets: a real
//! `GuardianLink` and `WardLink`, plus raw-socket fake peers that misbehave
//! in specific ways. Built only with `--features net`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use charter_crypto::{schnorr_sign, sha256};
use charter_link::{
    decode_frame, encode_frame, frame_len, guardian_proof_sign, guardian_proof_verify, public_key,
    sign_ack, verify_ack, ward_proof_sign, AckResult, DeliveryState, Frame, GuardianEvent,
    GuardianLink, GuardianLinkConfig, LocalOffer, SendState, WardIngest, WardLink, WardLinkConfig,
    MAX_FRAME,
};
use charter_primitives::{kinds, EventId, NostrEvent, PubKey, Sig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::runtime::Runtime;
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn secret(label: &str) -> [u8; 32] {
    sha256(label.as_bytes())
}

fn zsk(label: &str) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(secret(label))
}

fn pk(label: &str) -> PubKey {
    public_key(&secret(label)).unwrap()
}

fn rt() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// A validly signed gift wrap addressed to `recipient`.
fn wrap(recipient: &PubKey, n: u32) -> NostrEvent {
    wrap_c(recipient, n, format!("opaque-payload-{n}"))
}

/// [`wrap`] with the given content (for size tests).
fn wrap_c(recipient: &PubKey, n: u32, content: String) -> NostrEvent {
    let author = secret(&format!("ephemeral-{n}"));
    let mut ev = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey: public_key(&author).unwrap(),
        created_at: 1_761_400_000 + n as u64,
        kind: kinds::GIFT_WRAP,
        tags: vec![vec!["p".into(), recipient.to_hex()]],
        content,
        sig: Sig::from_bytes([0; 64]),
    };
    ev.id = charter_verify::event::event_id(&ev);
    ev.sig = Sig::from_bytes(schnorr_sign(&author, ev.id.as_bytes()).unwrap());
    ev
}

struct Recorder {
    got: Mutex<Vec<NostrEvent>>,
    result: AckResult,
}

impl Recorder {
    fn new(result: AckResult) -> Arc<Self> {
        Arc::new(Recorder {
            got: Mutex::new(Vec::new()),
            result,
        })
    }
    fn ids(&self) -> Vec<EventId> {
        self.got.lock().unwrap().iter().map(|w| w.id).collect()
    }
}

impl WardIngest for Recorder {
    fn ingest(
        &self,
        wrap: NostrEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AckResult> + Send>> {
        self.got.lock().unwrap().push(wrap);
        let r = self.result;
        Box::pin(async move { r })
    }
}

fn start_guardian(rt: &Runtime, g: &str, dir: &Path, allowed: Vec<PubKey>) -> Arc<GuardianLink> {
    GuardianLink::start(
        GuardianLinkConfig {
            guardian_sk: zsk(g),
            state_dir: dir.to_path_buf(),
            preferred_port: 0,
            allow_loopback: true,
        },
        allowed,
        rt.handle(),
    )
    .unwrap()
}

fn spawn_ward(
    rt: &Runtime,
    m: &str,
    pinned: PubKey,
    dir: &Path,
    ingest: Arc<Recorder>,
) -> Arc<WardLink> {
    WardLink::spawn(
        WardLinkConfig {
            machine_sk: zsk(m),
            guardian: pinned,
            state_dir: dir.to_path_buf(),
            allow_loopback: true,
        },
        ingest,
        rt.handle(),
    )
    .unwrap()
}

fn lo(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

fn wait_until(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

/// Gather guardian events (from this plain test thread) until `done`.
fn events_until(
    g: &GuardianLink,
    secs: u64,
    mut done: impl FnMut(&[GuardianEvent]) -> bool,
) -> Vec<GuardianEvent> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut all = Vec::new();
    while !done(&all) && Instant::now() < deadline {
        all.extend(g.next_events(Duration::from_millis(200)));
    }
    all
}

fn wraps_in(events: &[GuardianEvent]) -> Vec<(PubKey, EventId)> {
    events
        .iter()
        .filter_map(|e| match e {
            GuardianEvent::Wrap { machine, wrap } => Some((*machine, wrap.id)),
            _ => None,
        })
        .collect()
}

fn delivery_state(g: &GuardianLink, id: &EventId) -> Option<DeliveryState> {
    g.delivery(&[*id]).get(id).map(|r| r.state)
}

// --- raw peers -------------------------------------------------------------

#[derive(Debug)]
enum Recv {
    Frame(Frame),
    Closed,
    Timeout,
}

async fn recv(s: &mut TcpStream, secs: u64) -> Recv {
    let read = async {
        let mut prefix = [0u8; 4];
        if s.read_exact(&mut prefix).await.is_err() {
            return Recv::Closed;
        }
        let n = frame_len(prefix).expect("peer sent a bad length");
        let mut body = vec![0u8; n];
        if s.read_exact(&mut body).await.is_err() {
            return Recv::Closed;
        }
        Recv::Frame(decode_frame(&body).expect("peer sent a bad frame"))
    };
    tokio::time::timeout(Duration::from_secs(secs), read)
        .await
        .unwrap_or(Recv::Timeout)
}

async fn send(s: &mut TcpStream, f: &Frame) {
    let _ = s.write_all(&encode_frame(f)).await;
}

fn is_closed(r: &Recv) -> bool {
    matches!(r, Recv::Closed)
}

/// A raw ward: HELLO, verify CHALLENGE, AUTH. Returns the stream, the
/// guardian key, and what came after AUTH.
async fn raw_ward(port: u16, machine: &str, tamper: bool) -> (TcpStream, PubKey, Recv) {
    let s = TcpStream::connect(lo(port)).await.unwrap();
    raw_ward_on(s, machine, tamper).await
}

/// [`raw_ward`] over an already connected stream.
async fn raw_ward_on(mut s: TcpStream, machine: &str, tamper: bool) -> (TcpStream, PubKey, Recv) {
    let cw = [0x11u8; 32];
    send(
        &mut s,
        &Frame::Hello {
            v: 1,
            challenge: cw,
        },
    )
    .await;
    let (g, cg) = match recv(&mut s, 3).await {
        Recv::Frame(Frame::Challenge {
            v: 1,
            guardian,
            challenge,
            sig,
        }) => {
            assert!(guardian_proof_verify(&cw, &challenge, &guardian, &sig));
            (guardian, challenge)
        }
        other => panic!("expected CHALLENGE, got {other:?}"),
    };
    let msk = secret(machine);
    let mut sig = ward_proof_sign(&msk, &cw, &cg, &g).unwrap();
    if tamper {
        let mut b = *sig.as_bytes();
        b[10] ^= 0x40;
        sig = Sig::from_bytes(b);
    }
    send(
        &mut s,
        &Frame::Auth {
            machine: public_key(&msk).unwrap(),
            sig,
        },
    )
    .await;
    let after = recv(&mut s, 3).await;
    (s, g, after)
}

async fn raw_ward_ready(port: u16, machine: &str) -> (TcpStream, PubKey) {
    let (s, g, after) = raw_ward(port, machine, false).await;
    assert!(
        matches!(after, Recv::Frame(Frame::Ready { v: 1 })),
        "expected READY, got {after:?}"
    );
    (s, g)
}

/// Sync with the guardian: everything sent before this PING was processed
/// once its PONG arrives (sessions handle frames in order).
async fn barrier(s: &mut TcpStream, n: u64) -> Vec<Frame> {
    send(s, &Frame::Ping(n)).await;
    let mut before = Vec::new();
    loop {
        match recv(s, 3).await {
            Recv::Frame(Frame::Pong(m)) if m == n => return before,
            Recv::Frame(f) => before.push(f),
            other => panic!("expected PONG {n}, got {other:?}"),
        }
    }
}

/// A fake guardian listening on loopback: accept the ward and read HELLO.
async fn fake_accept(l: &TcpListener) -> (TcpStream, [u8; 32]) {
    let (mut s, _) = tokio::time::timeout(Duration::from_secs(5), l.accept())
        .await
        .expect("ward never connected")
        .unwrap();
    match recv(&mut s, 3).await {
        Recv::Frame(Frame::Hello { v: 1, challenge }) => (s, challenge),
        other => panic!("expected HELLO, got {other:?}"),
    }
}

/// Send a CHALLENGE signed by `signer` claiming to be `claimed`.
async fn fake_challenge(
    s: &mut TcpStream,
    cw: &[u8; 32],
    signer: &str,
    claimed: PubKey,
    tamper: bool,
) {
    let cg = [0x22u8; 32];
    let mut sig = guardian_proof_sign(&secret(signer), cw, &cg).unwrap();
    if tamper {
        let mut b = *sig.as_bytes();
        b[0] ^= 1;
        sig = Sig::from_bytes(b);
    }
    send(
        s,
        &Frame::Challenge {
            v: 1,
            guardian: claimed,
            challenge: cg,
            sig,
        },
    )
    .await;
}

/// A fake guardian that completes the handshake honestly.
async fn fake_guardian_ready(l: &TcpListener, g: &str) -> TcpStream {
    let (mut s, cw) = fake_accept(l).await;
    fake_challenge(&mut s, &cw, g, pk(g), false).await;
    match recv(&mut s, 3).await {
        Recv::Frame(Frame::Auth { .. }) => {}
        other => panic!("expected AUTH, got {other:?}"),
    }
    send(&mut s, &Frame::Ready { v: 1 }).await;
    s
}

fn offer(kind: u16, key: Option<&str>, w: NostrEvent) -> LocalOffer {
    LocalOffer {
        inner_kind: kind,
        key: key.map(str::to_owned),
        wrap: w,
    }
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

#[test]
fn guardian_to_ward_and_back_with_a_verified_applied_ack() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (g, m) = (pk("g"), pk("m"));
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", g, wdir.path(), rec.clone());
    ward.set_discovered(vec![lo(guardian.port())]);

    let ev = events_until(&guardian, 5, |e| {
        e.contains(&GuardianEvent::Session {
            machine: m,
            up: true,
        })
    });
    assert!(ev.contains(&GuardianEvent::Session {
        machine: m,
        up: true
    }));
    assert!(wait_until(5, || ward.snapshot().connected));
    let snap = ward.snapshot();
    assert_eq!(snap.sessions.len(), 1);
    assert_eq!(snap.sessions[0].addr, lo(guardian.port()).to_string());
    assert_eq!(snap.last_error, None);
    assert_eq!(guardian.sessions().len(), 1);
    assert_eq!(guardian.sessions()[0].machine, m);

    // Guardian → ward.
    let w1 = wrap(&m, 1);
    assert_eq!(guardian.send(&m, w1.clone()), charter_link::SendState::Sent);
    let ev = events_until(&guardian, 5, |e| {
        e.iter().any(|x| {
            matches!(
                x,
                GuardianEvent::Delivery {
                    state: DeliveryState::Acked(_),
                    ..
                }
            )
        })
    });
    let states: Vec<DeliveryState> = ev
        .iter()
        .filter_map(|x| match x {
            GuardianEvent::Delivery {
                wrap,
                machine,
                state,
            } if *wrap == w1.id && *machine == m => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![
            DeliveryState::Sent,
            DeliveryState::Received,
            DeliveryState::Acked(AckResult::Applied)
        ]
    );
    assert_eq!(rec.ids(), vec![w1.id]);
    let rec_entry = guardian.delivery(&[w1.id])[&w1.id].clone();
    assert_eq!(rec_entry.machine, m);
    assert_eq!(rec_entry.state, DeliveryState::Acked(AckResult::Applied));

    // Ward → guardian.
    let w2 = wrap(&g, 2);
    ward.sink()
        .offer(offer(kinds::CHARTER_DEVICE_REQUEST, None, w2.clone()));
    let ev = events_until(&guardian, 5, |e| !wraps_in(e).is_empty());
    assert_eq!(wraps_in(&ev), vec![(m, w2.id)]);
    let inbox = guardian.inbox();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].machine, m);
    assert_eq!(inbox[0].wrap, w2);
    // The guardian's OK takes it out of the ward outbox.
    assert!(wait_until(5, || ward.snapshot().outbox == 0));
    guardian.consume(&[w2.id]);
    assert!(guardian.inbox().is_empty());

    // Bridge shapes.
    let ev_json = serde_json::to_value(GuardianEvent::Delivery {
        wrap: w1.id,
        machine: m,
        state: DeliveryState::Acked(AckResult::Applied),
    })
    .unwrap();
    assert_eq!(ev_json["type"], "delivery");
    assert_eq!(ev_json["state"], "applied");
    let snap = serde_json::to_value(ward.snapshot()).unwrap();
    assert_eq!(snap["connected"], true);

    ward.shutdown();
    guardian.shutdown();
}

#[test]
fn send_rejects_bad_wraps_and_unpaired_recipients() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let stranger = pk("stranger");
    assert!(matches!(
        guardian.send(&stranger, wrap(&stranger, 1)),
        charter_link::SendState::Rejected(_)
    ));
    assert!(matches!(
        guardian.send(&m, wrap(&stranger, 2)),
        charter_link::SendState::Rejected(_)
    ));
    let mut forged = wrap(&m, 3);
    forged.content.push('!');
    assert!(matches!(
        guardian.send(&m, forged.clone()),
        charter_link::SendState::Rejected(_)
    ));
    assert!(guardian.delivery(&[forged.id]).is_empty());
}

// ---------------------------------------------------------------------------
// Handshake refusals
// ---------------------------------------------------------------------------

#[test]
fn ward_pinned_to_another_guardian_never_reveals_its_machine_key() {
    let rt = rt();
    let wdir = tempfile::tempdir().unwrap();
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", pk("pinned"), wdir.path(), rec.clone());
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    rt.block_on(async {
        let (mut s, cw) = fake_accept(&l).await;
        // A valid proof, but by a guardian this ward is not pinned to.
        fake_challenge(&mut s, &cw, "impostor", pk("impostor"), false).await;
        assert!(is_closed(&recv(&mut s, 3).await), "ward sent something");
    });
    assert!(wait_until(3, || ward
        .snapshot()
        .last_error
        .is_some_and(|e| e.contains("pinned"))));

    // Against a real guardian too: no session on either side.
    let gdir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "impostor", gdir.path(), vec![pk("m")]);
    ward.set_discovered(vec![lo(guardian.port())]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(guardian.sessions().is_empty());
    assert!(!ward.snapshot().connected);
    assert!(rec.ids().is_empty());
}

#[test]
fn guardian_proof_must_verify_and_must_not_claim_a_borrowed_key() {
    let rt = rt();
    let wdir = tempfile::tempdir().unwrap();
    let ward = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    rt.block_on(async {
        // Tampered signature, right key.
        let (mut s, cw) = fake_accept(&l).await;
        fake_challenge(&mut s, &cw, "g", pk("g"), true).await;
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    ward.kick();
    rt.block_on(async {
        // Claims the pinned key but signs with another.
        let (mut s, cw) = fake_accept(&l).await;
        fake_challenge(&mut s, &cw, "impostor", pk("g"), false).await;
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    assert!(!ward.snapshot().connected);
}

#[test]
fn guardian_refuses_a_machine_that_is_not_allowed() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    let port = guardian.port();
    rt.block_on(async {
        // The guardian still proves itself to a stranger, then shuts the door.
        let (_s, g, after) = raw_ward(port, "stranger", false).await;
        assert_eq!(g, pk("g"));
        assert!(is_closed(&after), "expected close, got {after:?}");
    });

    // A real ward that is not allowed gets no session either.
    let wdir = tempfile::tempdir().unwrap();
    let ward = spawn_ward(
        &rt,
        "stranger",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    ward.set_discovered(vec![lo(port)]);
    assert!(wait_until(3, || ward
        .snapshot()
        .last_error
        .is_some_and(|e| e.contains("not admitted"))));
    assert!(guardian.sessions().is_empty());
    assert!(!guardian
        .next_events(Duration::from_millis(100))
        .iter()
        .any(|e| matches!(e, GuardianEvent::Session { .. })));
}

#[test]
fn tampered_ward_proof_is_refused() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    rt.block_on(async {
        let (_s, _, after) = raw_ward(guardian.port(), "m", true).await;
        assert!(is_closed(&after), "expected close, got {after:?}");
    });
    assert!(guardian.sessions().is_empty());
}

#[test]
fn set_allowed_closes_sessions_no_longer_allowed() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let ward = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    ward.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || guardian.sessions().len() == 1));
    let _ = guardian.next_events(Duration::from_millis(10));
    guardian.set_allowed(vec![]);
    assert!(guardian.sessions().is_empty());
    let ev = events_until(&guardian, 2, |e| !e.is_empty());
    assert!(ev.contains(&GuardianEvent::Session {
        machine: m,
        up: false
    }));
    assert!(wait_until(5, || !ward.snapshot().connected));
}

#[test]
fn not_accepting_refuses_connections_and_keeps_the_queue() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", pk("g"), wdir.path(), rec.clone());
    ward.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || guardian.sessions().len() == 1));

    // Off the home network: sessions close, new connections are refused,
    // sends queue.
    guardian.set_accepting(false);
    assert!(guardian.sessions().is_empty());
    assert!(wait_until(5, || !ward.snapshot().connected));
    let port = guardian.port();
    let refused = rt.block_on(async {
        let mut s = TcpStream::connect(lo(port)).await.unwrap();
        recv(&mut s, 3).await
    });
    assert!(
        is_closed(&refused),
        "expected a closed socket, got {refused:?}"
    );
    let w = wrap(&m, 77);
    assert_eq!(guardian.send(&m, w.clone()), SendState::Queued);
    assert!(guardian.sessions().is_empty());

    // Back on: the ward reconnects and the queued wrap goes out.
    guardian.set_accepting(true);
    ward.kick();
    assert!(wait_until(10, || rec.ids().contains(&w.id)));
    assert_eq!(guardian.sessions().len(), 1);
}

// ---------------------------------------------------------------------------
// Bad frames and ordering
// ---------------------------------------------------------------------------

#[test]
fn zero_oversize_and_garbage_frames_close_the_guardian_connection() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    let port = guardian.port();
    rt.block_on(async {
        let mut cases: Vec<Vec<u8>> = vec![
            0u32.to_be_bytes().to_vec(),
            (MAX_FRAME as u32 + 1).to_be_bytes().to_vec(),
        ];
        let mut garbage = 7u32.to_be_bytes().to_vec();
        garbage.extend_from_slice(b"garbage");
        cases.push(garbage);
        let mut not_frame = 2u32.to_be_bytes().to_vec();
        not_frame.extend_from_slice(b"{}");
        cases.push(not_frame);
        for bytes in cases {
            let mut s = TcpStream::connect(lo(port)).await.unwrap();
            s.write_all(&bytes).await.unwrap();
            assert!(is_closed(&recv(&mut s, 3).await), "{bytes:?} did not close");
        }
        // After READY too.
        let (mut s, _) = raw_ward_ready(port, "m").await;
        s.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    assert!(wait_until(3, || guardian.sessions().is_empty()));
}

#[test]
fn bad_frames_close_the_ward_session() {
    let rt = rt();
    let wdir = tempfile::tempdir().unwrap();
    let ward = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    let bad: Vec<Vec<u8>> = vec![
        0u32.to_be_bytes().to_vec(),
        (MAX_FRAME as u32 + 1).to_be_bytes().to_vec(),
        [5u32.to_be_bytes().to_vec(), b"[nul]".to_vec()].concat(),
    ];
    for bytes in bad {
        ward.kick();
        rt.block_on(async {
            let mut s = fake_guardian_ready(&l, "g").await;
            assert!(wait_until_async(|| ward.snapshot().connected).await);
            s.write_all(&bytes).await.unwrap();
            assert!(is_closed(&recv(&mut s, 3).await), "{bytes:?} did not close");
        });
        assert!(wait_until(3, || !ward.snapshot().connected));
    }
}

async fn wait_until_async(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..250 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[test]
fn event_before_ready_closes_and_is_never_stored_by_the_guardian() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let g = pk("g");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    let port = guardian.port();
    rt.block_on(async {
        // EVENT instead of HELLO.
        let mut s = TcpStream::connect(lo(port)).await.unwrap();
        send(&mut s, &Frame::Event(wrap(&g, 1))).await;
        assert!(is_closed(&recv(&mut s, 3).await));
        // EVENT instead of AUTH.
        let mut s = TcpStream::connect(lo(port)).await.unwrap();
        send(
            &mut s,
            &Frame::Hello {
                v: 1,
                challenge: [3; 32],
            },
        )
        .await;
        assert!(matches!(
            recv(&mut s, 3).await,
            Recv::Frame(Frame::Challenge { .. })
        ));
        send(&mut s, &Frame::Event(wrap(&g, 2))).await;
        assert!(is_closed(&recv(&mut s, 3).await));
        // A wrong protocol version in HELLO.
        let mut s = TcpStream::connect(lo(port)).await.unwrap();
        send(
            &mut s,
            &Frame::Hello {
                v: 2,
                challenge: [3; 32],
            },
        )
        .await;
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    assert!(guardian.inbox().is_empty());
    assert!(wraps_in(&guardian.next_events(Duration::from_millis(200))).is_empty());
}

#[test]
fn event_before_ready_closes_and_is_never_ingested_by_the_ward() {
    let rt = rt();
    let wdir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", pk("g"), wdir.path(), rec.clone());
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    rt.block_on(async {
        // EVENT instead of CHALLENGE.
        let (mut s, _) = fake_accept(&l).await;
        send(&mut s, &Frame::Event(wrap(&m, 1))).await;
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    ward.kick();
    rt.block_on(async {
        // EVENT instead of READY.
        let (mut s, cw) = fake_accept(&l).await;
        fake_challenge(&mut s, &cw, "g", pk("g"), false).await;
        assert!(matches!(
            recv(&mut s, 3).await,
            Recv::Frame(Frame::Auth { .. })
        ));
        send(&mut s, &Frame::Event(wrap(&m, 2))).await;
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    assert!(rec.ids().is_empty());
    assert!(!ward.snapshot().connected);
}

#[test]
fn guardian_closes_a_session_over_its_frame_rate() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    let port = guardian.port();
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        let mut burst = Vec::new();
        for n in 0..620u64 {
            burst.extend_from_slice(&encode_frame(&Frame::Unknown(format!("X{n}"))));
        }
        s.write_all(&burst).await.unwrap();
        // Unknown frames get no reply, so the next thing is the close.
        assert!(is_closed(&recv(&mut s, 3).await));
    });
    assert!(wait_until(3, || guardian.sessions().is_empty()));
}

#[test]
fn ward_closes_a_session_over_its_frame_rate() {
    let rt = rt();
    let wdir = tempfile::tempdir().unwrap();
    let ward = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    rt.block_on(async {
        let mut s = fake_guardian_ready(&l, "g").await;
        let mut burst = Vec::new();
        for n in 0..620u64 {
            burst.extend_from_slice(&encode_frame(&Frame::Ping(n)));
        }
        s.write_all(&burst).await.unwrap();
        let mut pongs = 0;
        loop {
            match recv(&mut s, 3).await {
                Recv::Frame(Frame::Pong(_)) => pongs += 1,
                Recv::Closed => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(pongs <= 600, "{pongs} pongs");
    });
}

// ---------------------------------------------------------------------------
// Inbound validation and dedupe
// ---------------------------------------------------------------------------

#[test]
fn the_same_ward_wrap_twice_is_one_event_and_one_inbox_entry() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    let w = wrap(&pk("g"), 7);
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        send(&mut s, &Frame::Event(w.clone())).await;
        send(&mut s, &Frame::Event(w.clone())).await;
        let replies = barrier(&mut s, 1).await;
        let oks: Vec<(EventId, bool)> = replies
            .iter()
            .filter_map(|f| match f {
                Frame::Ok { wrap, accepted, .. } => Some((*wrap, *accepted)),
                _ => None,
            })
            .collect();
        assert_eq!(oks, vec![(w.id, true), (w.id, true)]);
    });
    let ev = events_until(&guardian, 1, |_| false);
    assert_eq!(wraps_in(&ev), vec![(m, w.id)]);
    assert_eq!(guardian.inbox().len(), 1);

    // Consumed, then the relay-era duplicate arrives again: still not stored.
    guardian.consume(&[w.id]);
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        send(&mut s, &Frame::Event(w.clone())).await;
        let replies = barrier(&mut s, 2).await;
        assert!(matches!(
            replies.as_slice(),
            [Frame::Ok { accepted: true, .. }]
        ));
    });
    assert!(guardian.inbox().is_empty());
    assert!(wraps_in(&guardian.next_events(Duration::from_millis(200))).is_empty());

    // And across a restart (the seen set is persisted).
    guardian.shutdown();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        send(&mut s, &Frame::Event(w.clone())).await;
        barrier(&mut s, 3).await;
    });
    assert!(guardian.inbox().is_empty());
}

#[test]
fn invalid_wraps_get_ok_false_and_go_nowhere() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        // Addressed to the ward itself, not the guardian.
        let wrong = wrap(&m, 1);
        send(&mut s, &Frame::Event(wrong.clone())).await;
        let mut forged = wrap(&pk("g"), 2);
        forged.content = "changed".into();
        send(&mut s, &Frame::Event(forged.clone())).await;
        let replies = barrier(&mut s, 1).await;
        assert_eq!(
            replies,
            vec![
                Frame::Ok {
                    wrap: wrong.id,
                    accepted: false,
                    reason: "invalid".into()
                },
                Frame::Ok {
                    wrap: forged.id,
                    accepted: false,
                    reason: "invalid".into()
                }
            ]
        );
    });
    assert!(guardian.inbox().is_empty());

    // The ward side: an invalid wrap is refused and not ingested; a valid
    // one is OK'd, ingested and answered with an ACK signed by the machine.
    let wdir = tempfile::tempdir().unwrap();
    let rec = Recorder::new(AckResult::Superseded);
    let ward = spawn_ward(&rt, "m", pk("g"), wdir.path(), rec.clone());
    let l = rt.block_on(TcpListener::bind(lo(0))).unwrap();
    ward.set_discovered(vec![lo(l.local_addr().unwrap().port())]);
    rt.block_on(async {
        let mut s = fake_guardian_ready(&l, "g").await;
        let bad = wrap(&pk("g"), 3);
        send(&mut s, &Frame::Event(bad.clone())).await;
        let good = wrap(&m, 4);
        send(&mut s, &Frame::Event(good.clone())).await;
        let replies = barrier(&mut s, 9).await;
        assert_eq!(replies.len(), 3, "{replies:?}");
        assert_eq!(
            replies[0],
            Frame::Ok {
                wrap: bad.id,
                accepted: false,
                reason: "invalid".into()
            }
        );
        assert!(matches!(&replies[1], Frame::Ok { wrap, accepted: true, .. } if *wrap == good.id));
        match &replies[2] {
            Frame::Ack(ack) => {
                assert_eq!(ack.wrap, good.id);
                assert_eq!(ack.result, AckResult::Superseded);
                assert!(verify_ack(ack, &pk("g"), &m));
            }
            other => panic!("expected ACK, got {other:?}"),
        }
    });
    assert_eq!(rec.ids(), vec![wrap(&m, 4).id]);
}

#[test]
fn acks_and_oks_count_only_from_the_machine_a_wrap_was_sent_to() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let (g, m1, m2) = (pk("g"), pk("m1"), pk("m2"));
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m1, m2]);
    let port = guardian.port();
    let (w1, w2) = (wrap(&m1, 1), wrap(&m2, 2));
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m1").await;
        assert!(wait_until_async(|| guardian.sessions().len() == 1).await);
        assert_eq!(
            guardian.send(&m1, w1.clone()),
            charter_link::SendState::Sent
        );
        assert_eq!(
            guardian.send(&m2, w2.clone()),
            charter_link::SendState::Queued
        );
        assert!(matches!(recv(&mut s, 3).await, Recv::Frame(Frame::Event(e)) if e.id == w1.id));

        // Signed by the wrong key.
        let by_stranger = sign_ack(&secret("stranger"), &g, &w1.id, AckResult::Applied, 5);
        send(&mut s, &Frame::Ack(by_stranger)).await;
        // Signed for another guardian.
        let other_g = sign_ack(&secret("m1"), &pk("other"), &w1.id, AckResult::Applied, 5);
        send(&mut s, &Frame::Ack(other_g)).await;
        // Validly signed by m1, but for a wrap sent to m2.
        let not_mine = sign_ack(&secret("m1"), &g, &w2.id, AckResult::Applied, 5);
        send(&mut s, &Frame::Ack(not_mine)).await;
        send(
            &mut s,
            &Frame::Ok {
                wrap: w2.id,
                accepted: true,
                reason: String::new(),
            },
        )
        .await;
        // Tampered timestamp.
        let mut tampered = sign_ack(&secret("m1"), &g, &w1.id, AckResult::Applied, 5);
        tampered.ts = 6;
        send(&mut s, &Frame::Ack(tampered)).await;
        barrier(&mut s, 1).await;
        assert_eq!(delivery_state(&guardian, &w1.id), Some(DeliveryState::Sent));
        assert_eq!(
            delivery_state(&guardian, &w2.id),
            Some(DeliveryState::Queued)
        );

        // The genuine article.
        let good = sign_ack(&secret("m1"), &g, &w1.id, AckResult::Unchanged, 5);
        send(&mut s, &Frame::Ack(good)).await;
        barrier(&mut s, 2).await;
    });
    assert_eq!(
        delivery_state(&guardian, &w1.id),
        Some(DeliveryState::Acked(AckResult::Unchanged))
    );
    assert_eq!(
        delivery_state(&guardian, &w2.id),
        Some(DeliveryState::Queued)
    );
}

// ---------------------------------------------------------------------------
// Persistence, restart and reconnection
// ---------------------------------------------------------------------------

#[test]
fn queued_sends_are_delivered_when_the_ward_connects_even_across_a_guardian_restart() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let (w1, w2) = (wrap(&m, 1), wrap(&m, 2));
    let g1 = start_guardian(&rt, "g", gdir.path(), vec![m]);
    assert_eq!(g1.send(&m, w1.clone()), charter_link::SendState::Queued);
    assert_eq!(delivery_state(&g1, &w1.id), Some(DeliveryState::Queued));
    g1.shutdown();
    drop(g1);

    let g2 = start_guardian(&rt, "g", gdir.path(), vec![m]);
    assert_eq!(delivery_state(&g2, &w1.id), Some(DeliveryState::Queued));
    assert_eq!(g2.send(&m, w2.clone()), charter_link::SendState::Queued);
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", pk("g"), wdir.path(), rec.clone());
    ward.set_discovered(vec![lo(g2.port())]);
    assert!(wait_until(5, || rec.ids().len() == 2));
    assert_eq!(rec.ids(), vec![w1.id, w2.id]);
    assert!(wait_until(5, || {
        delivery_state(&g2, &w1.id) == Some(DeliveryState::Acked(AckResult::Applied))
            && delivery_state(&g2, &w2.id) == Some(DeliveryState::Acked(AckResult::Applied))
    }));

    // Received wraps leave the queue: a later restart re-sends nothing.
    ward.shutdown();
    g2.shutdown();
    drop(g2);
    let g3 = start_guardian(&rt, "g", gdir.path(), vec![m]);
    assert_eq!(
        delivery_state(&g3, &w1.id),
        Some(DeliveryState::Acked(AckResult::Applied))
    );
    let rec2 = Recorder::new(AckResult::Applied);
    let wdir2 = tempfile::tempdir().unwrap();
    let ward2 = spawn_ward(&rt, "m", pk("g"), wdir2.path(), rec2.clone());
    ward2.set_discovered(vec![lo(g3.port())]);
    assert!(wait_until(5, || ward2.snapshot().connected));
    std::thread::sleep(Duration::from_millis(300));
    assert!(rec2.ids().is_empty());
}

fn age_outbox_entry(dir: &Path, id: &EventId, secs: u64) {
    let path = dir.join("link-outbox.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut hit = false;
    for e in v["entries"].as_array_mut().unwrap() {
        if e["wrap"]["id"] == id.to_hex() {
            let t = e["offeredAt"].as_u64().unwrap();
            e["offeredAt"] = (t - secs).into();
            hit = true;
        }
    }
    assert!(hit, "entry not in the outbox file");
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[test]
fn ward_outbox_survives_restart_expires_by_kind_and_replaces_status_by_key() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (g, m) = (pk("g"), pk("m"));
    let r_old = wrap(&g, 1);
    let a_old = wrap(&g, 2);
    let r_new = wrap(&g, 3);
    let (st1, st2, st3) = (wrap(&g, 4), wrap(&g, 5), wrap(&g, 6));
    {
        let ward = spawn_ward(&rt, "m", g, wdir.path(), Recorder::new(AckResult::Applied));
        let sink = ward.sink();
        sink.offer(offer(kinds::CHARTER_DEVICE_REQUEST, None, r_old.clone()));
        sink.offer(offer(kinds::CHARTER_DEVICE_AUDIT, None, a_old.clone()));
        sink.offer(offer(kinds::CHARTER_DEVICE_REQUEST, None, r_new.clone()));
        sink.offer(offer(kinds::CHARTER_DEVICE_STATUS, Some("s"), st1.clone()));
        sink.offer(offer(kinds::CHARTER_DEVICE_STATUS, Some("s"), st2.clone()));
        sink.offer(offer(kinds::CHARTER_DEVICE_STATUS, Some("t"), st3.clone()));
        // The same wrap again is not a second entry.
        sink.offer(offer(kinds::CHARTER_DEVICE_REQUEST, None, r_new.clone()));
        assert_eq!(ward.snapshot().outbox, 5);
        ward.shutdown();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(wdir.path().join("link-outbox.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The device was off for 25 hours: past the REQUEST TTL (24 h), within
    // the AUDIT TTL (48 h).
    age_outbox_entry(wdir.path(), &r_old.id, 25 * 3600);
    age_outbox_entry(wdir.path(), &a_old.id, 25 * 3600);

    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let ward = spawn_ward(&rt, "m", g, wdir.path(), Recorder::new(AckResult::Applied));
    assert_eq!(ward.snapshot().outbox, 4);
    ward.set_discovered(vec![lo(guardian.port())]);
    let ev = events_until(&guardian, 5, |e| wraps_in(e).len() >= 4);
    std::thread::sleep(Duration::from_millis(300));
    let mut got = wraps_in(&ev);
    got.extend(wraps_in(&guardian.next_events(Duration::from_millis(100))));
    let ids: Vec<EventId> = got.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids, vec![a_old.id, r_new.id, st2.id, st3.id]);
    assert!(got.iter().all(|(mm, _)| *mm == m));
    assert!(wait_until(5, || ward.snapshot().outbox == 0));
}

#[test]
fn ward_reconnects_after_the_guardian_restarts_on_a_new_port() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let g1 = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let p1 = g1.port();
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", pk("g"), wdir.path(), rec.clone());
    ward.set_discovered(vec![lo(p1)]);
    assert!(wait_until(5, || g1.sessions().len() == 1));
    g1.shutdown();
    drop(g1);
    assert!(wait_until(5, || !ward.snapshot().connected));

    // Keep the old listener's port busy so the restart cannot reuse it.
    let _squat = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, p1)).ok();
    let g2 = start_guardian(&rt, "g", gdir.path(), vec![m]);
    assert_ne!(g2.port(), p1);
    ward.set_discovered(vec![lo(g2.port())]);
    assert!(wait_until(5, || g2.sessions().len() == 1));
    let w = wrap(&m, 1);
    assert_eq!(g2.send(&m, w.clone()), charter_link::SendState::Sent);
    assert!(wait_until(5, || delivery_state(&g2, &w.id)
        == Some(DeliveryState::Acked(AckResult::Applied))));
    assert_eq!(rec.ids(), vec![w.id]);
}

#[test]
fn a_restarted_ward_uses_the_remembered_guardian_address() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let w1 = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    w1.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || w1.snapshot().connected));
    w1.shutdown();
    drop(w1);
    assert!(wait_until(5, || guardian.sessions().is_empty()));

    // No discovery at all this time.
    let w2 = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    let snap = w2.snapshot();
    assert_eq!(snap.remembered, Some(lo(guardian.port()).to_string()));
    assert_eq!(snap.candidates, 1);
    assert!(wait_until(5, || w2.snapshot().connected));
    assert_eq!(guardian.sessions().len(), 1);

    // A remembered address for a different pinned guardian is not used.
    w2.shutdown();
    drop(w2);
    let w3 = spawn_ward(
        &rt,
        "m",
        pk("other"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    assert_eq!(w3.snapshot().remembered, None);
}

/// L8: loopback is opt-in. A production ward (`allow_loopback: false`) takes
/// no loopback candidate from discovery, and a remembered loopback guardian
/// address is not used either.
#[test]
fn a_ward_without_loopback_refuses_loopback_candidates_and_remembered_peers() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    // A test ward (loopback allowed) connects and remembers 127.0.0.1.
    let w1 = spawn_ward(
        &rt,
        "m",
        pk("g"),
        wdir.path(),
        Recorder::new(AckResult::Applied),
    );
    w1.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || w1.snapshot().connected));
    w1.shutdown();
    drop(w1);
    assert!(wait_until(5, || guardian.sessions().is_empty()));

    let strict = WardLink::spawn(
        WardLinkConfig {
            machine_sk: zsk("m"),
            guardian: pk("g"),
            state_dir: wdir.path().to_path_buf(),
            allow_loopback: false,
        },
        Recorder::new(AckResult::Applied),
        rt.handle(),
    )
    .unwrap();
    assert_eq!(
        strict.snapshot().remembered,
        None,
        "a remembered loopback peer must not be used"
    );
    strict.set_discovered(vec![lo(guardian.port()), lo(1)]);
    assert_eq!(strict.snapshot().candidates, 0);
    // A private address is still taken.
    strict.set_discovered(vec![
        lo(guardian.port()),
        "192.168.1.5:47615".parse().unwrap(),
    ]);
    assert_eq!(strict.snapshot().candidates, 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(guardian.sessions().is_empty(), "no session over loopback");
    strict.shutdown();
}

/// What a platform deletes on unpair covers everything the link wrote.
#[test]
fn state_files_cover_every_file_the_ward_link_writes() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let g = pk("g");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let ward = spawn_ward(&rt, "m", g, wdir.path(), Recorder::new(AckResult::Applied));
    ward.sink()
        .offer(offer(kinds::CHARTER_DEVICE_AUDIT, None, wrap(&g, 1)));
    ward.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || ward.snapshot().connected));
    let outbox = wdir.path().join("link-outbox.json");
    let peer = wdir.path().join("link-peer.json");
    assert!(wait_until(5, || outbox.exists() && peer.exists()));
    ward.shutdown();
    let listed = WardLink::state_files(wdir.path());
    for entry in std::fs::read_dir(wdir.path()).unwrap() {
        let path = entry.unwrap().path();
        assert!(listed.contains(&path), "{path:?} is not in state_files");
    }
}

#[test]
fn preferred_port_falls_back_when_taken() {
    let rt = rt();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let cfg = |dir: &Path, port: u16| GuardianLinkConfig {
        guardian_sk: zsk("g"),
        state_dir: dir.to_path_buf(),
        preferred_port: port,
        allow_loopback: true,
    };
    let a = GuardianLink::start(
        cfg(d1.path(), charter_link::PREFERRED_PORT),
        vec![],
        rt.handle(),
    )
    .unwrap();
    let b = GuardianLink::start(cfg(d2.path(), a.port()), vec![], rt.handle()).unwrap();
    assert_ne!(a.port(), b.port());
}

#[test]
fn loopback_peers_are_refused_unless_allowed() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = GuardianLink::start(
        GuardianLinkConfig {
            guardian_sk: zsk("g"),
            state_dir: dir.path().to_path_buf(),
            preferred_port: 0,
            allow_loopback: false,
        },
        vec![pk("m")],
        rt.handle(),
    )
    .unwrap();
    rt.block_on(async {
        let mut s = TcpStream::connect(lo(guardian.port())).await.unwrap();
        send(
            &mut s,
            &Frame::Hello {
                v: 1,
                challenge: [1; 32],
            },
        )
        .await;
        // Closed at once: not even a CHALLENGE.
        assert!(is_closed(&recv(&mut s, 3).await));
    });
}

// ---------------------------------------------------------------------------
// Review regressions: lifecycle, bounds and persistence
// ---------------------------------------------------------------------------

/// An ingest that takes `delay`, counting starts and completions.
struct Slow {
    started: AtomicUsize,
    done: Arc<AtomicUsize>,
    delay: Duration,
}

impl WardIngest for Slow {
    fn ingest(
        &self,
        _wrap: NostrEvent,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AckResult> + Send>> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let (delay, done) = (self.delay, self.done.clone());
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            done.fetch_add(1, Ordering::SeqCst);
            AckResult::Applied
        })
    }
}

fn session_flips(events: &[GuardianEvent]) -> (usize, usize) {
    let ups = events
        .iter()
        .filter(|e| matches!(e, GuardianEvent::Session { up: true, .. }))
        .count();
    let downs = events
        .iter()
        .filter(|e| matches!(e, GuardianEvent::Session { up: false, .. }))
        .count();
    (ups, downs)
}

/// A TCP connection to the guardian from a chosen loopback source address.
async fn connect_from(src: Ipv4Addr, port: u16, recv_buf: Option<u32>) -> TcpStream {
    let sock = TcpSocket::new_v4().unwrap();
    if let Some(n) = recv_buf {
        sock.set_recv_buffer_size(n).unwrap();
    }
    sock.bind(SocketAddr::V4(SocketAddrV4::new(src, 0)))
        .unwrap();
    sock.connect(SocketAddr::V4(lo(port))).await.unwrap()
}

#[test]
fn an_ingest_in_flight_finishes_when_the_ward_shuts_down() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    let slow = Arc::new(Slow {
        started: AtomicUsize::new(0),
        done: Arc::new(AtomicUsize::new(0)),
        delay: Duration::from_millis(1500),
    });
    let ward = WardLink::spawn(
        WardLinkConfig {
            machine_sk: zsk("m"),
            guardian: pk("g"),
            state_dir: wdir.path().to_path_buf(),
            allow_loopback: true,
        },
        slow.clone(),
        rt.handle(),
    )
    .unwrap();
    ward.set_discovered(vec![lo(guardian.port())]);
    assert!(wait_until(5, || !guardian.sessions().is_empty()));
    let w = wrap(&m, 1);
    assert_eq!(guardian.send(&m, w.clone()), SendState::Sent);
    assert!(wait_until(5, || slow.started.load(Ordering::SeqCst) == 1));
    // The session ends while the broker is part way through.
    ward.shutdown();
    assert!(
        wait_until(5, || slow.done.load(Ordering::SeqCst) == 1),
        "the ingest was dropped mid-way"
    );
    assert_eq!(slow.started.load(Ordering::SeqCst), 1);
    // The session is gone, so its ACK is not sent: receipt only.
    assert_eq!(
        delivery_state(&guardian, &w.id),
        Some(DeliveryState::Received)
    );
}

#[test]
fn an_oversize_outbox_offer_is_refused_and_the_rest_still_flows() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (g, m) = (pk("g"), pk("m"));
    let ward = spawn_ward(&rt, "m", g, wdir.path(), Recorder::new(AckResult::Applied));
    let sink = ward.sink();
    let big = wrap_c(&g, 1, "a".repeat(MAX_FRAME));
    let small = wrap(&g, 2);
    sink.offer(offer(kinds::CHARTER_DEVICE_STATUS, Some("s"), big));
    sink.offer(offer(kinds::CHARTER_DEVICE_REQUEST, None, small.clone()));
    assert_eq!(ward.snapshot().outbox, 1, "the oversize wrap was held");

    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    ward.set_discovered(vec![lo(guardian.port())]);
    let mut ev = events_until(&guardian, 5, |e| {
        wraps_in(e).iter().any(|(_, id)| *id == small.id)
    });
    assert_eq!(wraps_in(&ev), vec![(m, small.id)]);
    assert!(wait_until(5, || ward.snapshot().outbox == 0));
    std::thread::sleep(Duration::from_secs(3));
    ev.extend(guardian.next_events(Duration::from_millis(100)));
    assert_eq!(session_flips(&ev), (1, 0), "the session churned");
    assert_eq!(ward.snapshot().last_error, None);
}

#[test]
fn a_stored_oversize_outbox_entry_is_dropped_at_start() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (g, m) = (pk("g"), pk("m"));
    let big = wrap_c(&g, 1, "a".repeat(MAX_FRAME));
    let small = wrap(&g, 2);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let file = serde_json::json!({
        "v": 1,
        "entries": [
            {"innerKind": kinds::CHARTER_DEVICE_STATUS, "key": "s", "offeredAt": now, "wrap": big},
            {"innerKind": kinds::CHARTER_DEVICE_REQUEST, "key": null, "offeredAt": now, "wrap": small},
        ],
    });
    std::fs::write(
        wdir.path().join("link-outbox.json"),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    let ward = spawn_ward(&rt, "m", g, wdir.path(), Recorder::new(AckResult::Applied));
    assert_eq!(ward.snapshot().outbox, 1);
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(wdir.path().join("link-outbox.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["entries"].as_array().unwrap().len(), 1);

    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    ward.set_discovered(vec![lo(guardian.port())]);
    let ev = events_until(&guardian, 5, |e| !wraps_in(e).is_empty());
    assert_eq!(wraps_in(&ev), vec![(m, small.id)]);
}

#[test]
fn a_full_backlog_both_ways_flushes_on_one_session() {
    let rt = rt();
    let (gdir, wdir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (g, m) = (pk("g"), pk("m"));
    let guardian = start_guardian(&rt, "g", gdir.path(), vec![m]);
    // A full guardian queue for the ward ...
    let down: Vec<NostrEvent> = (0..64).map(|n| wrap(&m, n)).collect();
    for w in &down {
        assert_eq!(guardian.send(&m, w.clone()), SendState::Queued);
    }
    // ... and a full ward outbox in every retention class.
    let rec = Recorder::new(AckResult::Applied);
    let ward = spawn_ward(&rt, "m", g, wdir.path(), rec.clone());
    let sink = ward.sink();
    let mut up = Vec::new();
    for i in 0..64u32 {
        for (c, kind) in [
            kinds::CHARTER_DEVICE_REQUEST,
            kinds::CHARTER_DEVICE_AUDIT,
            kinds::CHARTER_DEVICE_STATUS,
            kinds::CHARTER_DEVICE_USAGE_SYNC,
        ]
        .into_iter()
        .enumerate()
        {
            let w = wrap(&g, 1000 + i * 4 + c as u32);
            let key = format!("s{i}");
            let key = (kind == kinds::CHARTER_DEVICE_STATUS).then_some(key.as_str());
            sink.offer(offer(kind, key, w.clone()));
            up.push(w.id);
        }
    }
    assert_eq!(ward.snapshot().outbox, 256);
    let _ = guardian.next_events(Duration::from_millis(10));

    ward.set_discovered(vec![lo(guardian.port())]);
    let acked = |e: &[GuardianEvent]| {
        e.iter()
            .filter(|e| {
                matches!(
                    e,
                    GuardianEvent::Delivery {
                        state: DeliveryState::Acked(_),
                        ..
                    }
                )
            })
            .count()
    };
    let mut ev = events_until(&guardian, 30, |e| {
        wraps_in(e).len() >= 256 && acked(e) >= 64
    });
    std::thread::sleep(Duration::from_millis(500));
    ev.extend(guardian.next_events(Duration::from_millis(100)));

    assert_eq!(session_flips(&ev), (1, 0), "the session churned");
    let got: Vec<EventId> = wraps_in(&ev).into_iter().map(|(_, id)| id).collect();
    assert_eq!(got.len(), 256);
    let mut sorted_got = got.clone();
    sorted_got.sort();
    let mut sorted_up = up.clone();
    sorted_up.sort();
    assert_eq!(sorted_got, sorted_up);
    // Each guardian wrap ingested once, and every one acknowledged.
    assert_eq!(rec.ids().len(), 64);
    for w in &down {
        assert_eq!(
            delivery_state(&guardian, &w.id),
            Some(DeliveryState::Acked(AckResult::Applied))
        );
    }
    assert!(wait_until(5, || ward.snapshot().outbox == 0));
}

#[test]
fn inbox_and_pending_events_are_bounded_by_bytes() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let (g, m) = (pk("g"), pk("m"));
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    let wraps: Vec<NostrEvent> = (0..40)
        .map(|n| wrap_c(&g, n, "c".repeat(250_000)))
        .collect();
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        for w in &wraps {
            send(&mut s, &Frame::Event(w.clone())).await;
            match recv(&mut s, 5).await {
                Recv::Frame(Frame::Ok { wrap, accepted, .. }) => {
                    assert_eq!(wrap, w.id);
                    assert!(accepted);
                }
                other => panic!("expected OK, got {other:?}"),
            }
        }
    });
    const CAP: usize = 8 * 1024 * 1024;
    let inbox = guardian.inbox();
    let held: usize = inbox.iter().map(|e| e.wrap.content.len()).sum();
    assert!(held <= CAP, "inbox holds {held} bytes");
    assert!(inbox.len() < wraps.len());
    assert_eq!(inbox.last().map(|e| e.id), Some(wraps[39].id));
    let on_disk = std::fs::metadata(dir.path().join("link-inbox.json"))
        .unwrap()
        .len() as usize;
    assert!(on_disk <= CAP + 64 * 1024, "inbox file is {on_disk} bytes");

    let ev = guardian.next_events(Duration::from_millis(100));
    let pending: usize = ev
        .iter()
        .filter_map(|e| match e {
            GuardianEvent::Wrap { wrap, .. } => Some(wrap.content.len()),
            _ => None,
        })
        .sum();
    assert!(pending <= CAP, "events hold {pending} bytes of wraps");
    assert!(wraps_in(&ev).contains(&(m, wraps[39].id)));
}

#[test]
fn a_sent_wrap_is_resent_after_its_session_dies_unanswered() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    let w = wrap(&m, 1);
    rt.block_on(async {
        let (s, _) = raw_ward_ready(port, "m").await;
        assert!(wait_until_async(|| guardian.sessions().len() == 1).await);
        assert_eq!(guardian.send(&m, w.clone()), SendState::Sent);
        // Gone before reading it or answering OK.
        drop(s);
        assert!(wait_until_async(|| guardian.sessions().is_empty()).await);

        let (mut s, _) = raw_ward_ready(port, "m").await;
        match recv(&mut s, 3).await {
            Recv::Frame(Frame::Event(e)) => assert_eq!(e.id, w.id),
            other => panic!("expected the wrap again, got {other:?}"),
        }
        send(
            &mut s,
            &Frame::Ok {
                wrap: w.id,
                accepted: true,
                reason: String::new(),
            },
        )
        .await;
        barrier(&mut s, 1).await;
    });
    assert_eq!(
        delivery_state(&guardian, &w.id),
        Some(DeliveryState::Received)
    );
    // Answered: a later session gets nothing.
    rt.block_on(async {
        let (mut s, _) = raw_ward_ready(port, "m").await;
        assert!(barrier(&mut s, 2).await.is_empty());
    });
}

#[test]
fn a_wrap_queued_while_the_session_is_up_is_flushed_on_that_session() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let port = guardian.port();
    let late = wrap(&m, 99);
    rt.block_on(async {
        // A small receive buffer, so the guardian's socket fills quickly.
        let s = connect_from(Ipv4Addr::LOCALHOST, port, Some(4096)).await;
        let (mut s, _, after) = raw_ward_on(s, "m", false).await;
        assert!(matches!(after, Recv::Frame(Frame::Ready { v: 1 })));
        assert!(wait_until_async(|| guardian.sessions().len() == 1).await);
        // PONGs pile up in the session's channel while large wraps keep its
        // writer blocked on the socket: send wraps (each normally written
        // at once) until the channel is full and a send is only queued. The
        // count needed depends on the kernel's socket buffer sizes.
        let mut burst = Vec::new();
        for n in 0..300u64 {
            burst.extend_from_slice(&encode_frame(&Frame::Ping(n)));
        }
        let mut pinged = false;
        for n in 0..60 {
            let w = wrap_c(&m, n, "b".repeat(250_000));
            if guardian.send(&m, w) == SendState::Queued {
                break;
            }
            if n == 15 {
                s.write_all(&burst).await.unwrap();
                pinged = true;
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
        assert!(pinged);
        assert_eq!(
            guardian.send(&m, late.clone()),
            SendState::Queued,
            "the session channel never filled (socket buffers above ~15 MB?)"
        );
        // Reading again lets the session drain: the late wrap follows on
        // this same session, without a reconnect.
        loop {
            match recv(&mut s, 5).await {
                Recv::Frame(Frame::Event(e)) if e.id == late.id => break,
                Recv::Frame(Frame::Event(_) | Frame::Pong(_)) => {}
                other => panic!("expected the late wrap, got {other:?}"),
            }
        }
        // Still the one session it started on.
        assert_eq!(guardian.sessions().len(), 1);
    });
    assert_eq!(
        delivery_state(&guardian, &late.id),
        Some(DeliveryState::Sent)
    );
}

#[test]
fn a_corrupt_state_file_is_moved_aside_not_overwritten() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let bad = b"{\"v\":1,\"queue\":[TRUNCATED".to_vec();
    std::fs::write(dir.path().join("link-outbound.json"), &bad).unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    assert_eq!(guardian.send(&m, wrap(&m, 1)), SendState::Queued);
    let aside: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("link-outbound.json.corrupt-")
        })
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(std::fs::read(&aside[0]).unwrap(), bad);
    let now: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("link-outbound.json")).unwrap())
            .unwrap();
    assert_eq!(now["queue"].as_array().unwrap().len(), 1);
}

#[test]
fn unauthenticated_connections_are_capped_per_source_address() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let guardian = start_guardian(&rt, "g", dir.path(), vec![pk("m")]);
    let port = guardian.port();
    rt.block_on(async {
        // Two silent connections from one address hold its slots ...
        let _a = connect_from(Ipv4Addr::LOCALHOST, port, None).await;
        let _b = connect_from(Ipv4Addr::LOCALHOST, port, None).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // ... so a third from it is closed at once ...
        let mut c = connect_from(Ipv4Addr::LOCALHOST, port, None).await;
        send(
            &mut c,
            &Frame::Hello {
                v: 1,
                challenge: [1; 32],
            },
        )
        .await;
        assert!(is_closed(&recv(&mut c, 1).await));
        // ... while a ward at another address still gets in.
        let s = connect_from(Ipv4Addr::new(127, 0, 0, 2), port, None).await;
        let (_s, _, after) = raw_ward_on(s, "m", false).await;
        assert!(matches!(after, Recv::Frame(Frame::Ready { v: 1 })));
    });
}

#[test]
fn a_machine_at_the_global_session_cap_replaces_its_own_oldest_session() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let names = ["m1", "m2", "m3", "m4", "m5"];
    let guardian = start_guardian(&rt, "g", dir.path(), names.iter().map(|n| pk(n)).collect());
    let port = guardian.port();
    rt.block_on(async {
        let mut held = Vec::new();
        for n in &names[..4] {
            for _ in 0..4 {
                held.push(raw_ward_ready(port, n).await.0);
            }
        }
        assert!(wait_until_async(|| guardian.sessions().len() == 16).await);
        // A fifth session from m1 (its old ones gone stale) is admitted.
        held.push(raw_ward_ready(port, "m1").await.0);
        assert!(wait_until_async(|| guardian.sessions().len() == 16).await);
        // A machine with no session to give up is still refused at the cap.
        let (_s, _, after) = raw_ward(port, "m5", false).await;
        assert!(is_closed(&after), "expected a close, got {after:?}");
    });
}

#[test]
fn a_wrap_dropped_from_a_full_queue_is_recorded_rejected() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let m = pk("m");
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m]);
    let wraps: Vec<NostrEvent> = (0..65).map(|n| wrap(&m, n)).collect();
    for w in &wraps {
        assert_eq!(guardian.send(&m, w.clone()), SendState::Queued);
    }
    assert_eq!(
        delivery_state(&guardian, &wraps[0].id),
        Some(DeliveryState::Rejected)
    );
    for w in &wraps[1..] {
        assert_eq!(
            delivery_state(&guardian, &w.id),
            Some(DeliveryState::Queued)
        );
    }
}

#[test]
fn one_ward_flooding_the_inbox_does_not_evict_another() {
    let rt = rt();
    let dir = tempfile::tempdir().unwrap();
    let (g, m1, m2) = (pk("g"), pk("m1"), pk("m2"));
    let guardian = start_guardian(&rt, "g", dir.path(), vec![m1, m2]);
    let port = guardian.port();
    let mine: Vec<NostrEvent> = (0..10).map(|n| wrap(&g, n)).collect();
    rt.block_on(async {
        let (mut a, _) = raw_ward_ready(port, "m1").await;
        for w in &mine {
            send(&mut a, &Frame::Event(w.clone())).await;
        }
        barrier(&mut a, 1).await;
        let (mut b, _) = raw_ward_ready(port, "m2").await;
        for n in 0..300 {
            send(&mut b, &Frame::Event(wrap(&g, 1000 + n))).await;
        }
        barrier(&mut b, 2).await;
    });
    let inbox = guardian.inbox();
    assert!(inbox.len() <= 256);
    let kept: Vec<EventId> = inbox
        .iter()
        .filter(|e| e.machine == m1)
        .map(|e| e.id)
        .collect();
    assert_eq!(kept, mine.iter().map(|w| w.id).collect::<Vec<_>>());
}
