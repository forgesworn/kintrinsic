//! Local link integration tests over real loopback sockets: a real
//! `GuardianLink` and `WardLink`, plus raw-socket fake peers that misbehave
//! in specific ways. Built only with `--features net`.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
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
use tokio::net::{TcpListener, TcpStream};
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
    let author = secret(&format!("ephemeral-{n}"));
    let mut ev = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey: public_key(&author).unwrap(),
        created_at: 1_761_400_000 + n as u64,
        kind: kinds::GIFT_WRAP,
        tags: vec![vec!["p".into(), recipient.to_hex()]],
        content: format!("opaque-payload-{n}"),
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
    let mut s = TcpStream::connect(lo(port)).await.unwrap();
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
        for n in 0..130u64 {
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
        for n in 0..130u64 {
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
        assert!(pongs <= 120, "{pongs} pongs");
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
