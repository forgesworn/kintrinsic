//! Local link (v1), composed on the ward side the way `charterd` composes it,
//! over real loopback sockets: the real `Broker` over a real
//! `CharterTransport` (NIP-59, NIP-44) whose relay is scriptable (every relay
//! unreachable, or a poll that never returns), a real `WardLink` ingesting
//! through charterd's own `BrokerIngest`, its outbox bound through the same
//! `LateSink` the daemon uses, and a real `GuardianLink` standing in for the
//! carrier. What is proven here is the ward's half of the design note's
//! acceptance cases: a change over the link applies (and is ACKed truthfully)
//! with no relay at all, a request reaches the guardian with no relay, a
//! duplicate from the other route changes nothing, and a relay poll that is
//! stuck does not hold a local change up.

#![cfg(feature = "mock")]

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use charter_link::{
    AckResult, DeliveryState, GuardianLink, GuardianLinkConfig, SendState, WardIngest, WardLink,
    WardLinkConfig,
};
use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_proto::{ClauseKind, OpType};
use charter_spine::enactors::time_extend::TimeExtendEnactor;
use charter_spine::enforcer_runtime::EnforcerRuntime;
use charter_spine::multi_child::ExtensionInbox;
use charter_spine::ports::NullEventSink;
use charter_spine::{Broker, CharterTransportAdapter, EnactorRegistry, RequestState};
use charter_sys::persistence::{ChildClauseStore, PairingStore};
use charter_sys::relay::{
    Filter, MockRelayTransport, PublishOutcome, RelayIoError, RelayTransport, RelayUrl,
};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, GrantBuilder, TestGuardian};
use charterd::local_link::{BrokerIngest, BrokerPoller, LateSink, LinkControl};
use serde_json::json;
use zeroize::Zeroizing;

const NOW: u64 = 1_700_001_000;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);
/// `TestGuardian::new()` signs with the key whose low byte is 0x11.
const GUARDIAN_SEED: u8 = 0x11;
const RELAY: &str = "wss://r";

fn secret(b: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = b;
    s
}

fn machine_sk() -> [u8; 32] {
    secret(0x22)
}

fn machine_pk() -> PubKey {
    PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk()).unwrap())
}

// ---------------------------------------------------------------------------
// A relay the test can take down or wedge
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RelayState {
    /// Every relay unreachable: publishes fail, queries report Unreachable.
    down: AtomicBool,
    /// Queries never return until cleared (a wedged relay connection).
    stuck: AtomicBool,
    queries: AtomicU32,
}

struct TestRelay {
    store: MockRelayTransport,
    st: Arc<RelayState>,
}

#[async_trait]
impl RelayTransport for TestRelay {
    async fn publish(
        &self,
        relays: &[RelayUrl],
        ev: NostrEvent,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        if self.st.down.load(Ordering::SeqCst) {
            return relays
                .iter()
                .map(|r| (r.clone(), PublishOutcome::Failed("unreachable".into())))
                .collect();
        }
        self.store.publish(relays, ev).await
    }

    async fn query(
        &self,
        relays: &[RelayUrl],
        filter: Filter,
    ) -> Result<Vec<NostrEvent>, RelayIoError> {
        self.st.queries.fetch_add(1, Ordering::SeqCst);
        while self.st.stuck.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self.st.down.load(Ordering::SeqCst) {
            return Err(RelayIoError::Unreachable("no relay reachable".into()));
        }
        self.store.query(relays, filter).await
    }
}

type Facade = CharterTransportAdapter<TestRelay, ScriptedEntropy>;
type B = Broker<MockSystem, Facade, ScriptedEntropy>;

// ---------------------------------------------------------------------------
// The rig: ward broker + ward link + guardian link on loopback
// ---------------------------------------------------------------------------

struct Rig {
    guardian: TestGuardian,
    broker: Arc<B>,
    control: Arc<LinkControl>,
    relay: MockRelayTransport,
    st: Arc<RelayState>,
    carrier: Arc<GuardianLink>,
    ward: Arc<WardLink>,
    dirs: Vec<PathBuf>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.ward.shutdown();
        self.carrier.shutdown();
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("charterd-local-link-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn rig(name: &str, enactors: EnactorRegistry) -> Rig {
    let guardian = TestGuardian::new();
    assert_eq!(
        guardian.pubkey(),
        PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret(GUARDIAN_SEED)).unwrap()),
        "the carrier holds the same guardian key the clauses are signed with"
    );
    let st = Arc::new(RelayState::default());
    st.down.store(true, Ordering::SeqCst);
    let relay = MockRelayTransport::new();
    let late = LateSink::new();
    let transport = CharterTransport::new(
        TestRelay {
            store: relay.clone(),
            st: st.clone(),
        },
        ScriptedEntropy::new(7),
        machine_sk(),
        guardian.pubkey(),
        vec![RELAY.into()],
    )
    .with_local_sink(late.as_sink());
    let broker = Arc::new(Broker::new(
        MockSystem::new(NOW),
        CharterTransportAdapter::new(transport),
        ScriptedEntropy::new(1),
        enactors,
        Box::new(NullEventSink),
        SUBJECT,
    ));
    let control = LinkControl::new();

    let (gdir, wdir) = (
        fresh_dir(&format!("{name}-g")),
        fresh_dir(&format!("{name}-w")),
    );
    let rt = tokio::runtime::Handle::current();
    let carrier = GuardianLink::start(
        GuardianLinkConfig {
            guardian_sk: Zeroizing::new(secret(GUARDIAN_SEED)),
            state_dir: gdir.clone(),
            preferred_port: 0,
            allow_loopback: true,
        },
        vec![machine_pk()],
        &rt,
    )
    .unwrap();
    let ward = WardLink::spawn(
        WardLinkConfig {
            machine_sk: Zeroizing::new(machine_sk()),
            guardian: guardian.pubkey(),
            state_dir: wdir.clone(),
            allow_loopback: true,
            prepare_socket: None,
        },
        BrokerIngest::new(broker.clone(), control.clone()),
        &rt,
    )
    .unwrap();
    assert!(late.bind(ward.sink()));
    ward.set_discovered(vec![SocketAddrV4::new(Ipv4Addr::LOCALHOST, carrier.port())]);
    assert!(
        eventually(5, || ward.snapshot().connected
            && !carrier.sessions().is_empty())
        .await,
        "the ward connects out to the carrier"
    );
    Rig {
        guardian,
        broker,
        control,
        relay,
        st,
        carrier,
        ward,
        dirs: vec![gdir, wdir],
    }
}

async fn eventually(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}

impl Rig {
    /// The guardian gift-wraps `ev` to the ward. A different `salt` is a
    /// different wrap (new ephemeral key, new id) of the same inner event.
    fn wrap(&self, ev: &NostrEvent, salt: u8) -> NostrEvent {
        nip59::wrap(
            &Rumor::from_signed_event(ev),
            &secret(GUARDIAN_SEED),
            machine_pk().as_bytes(),
            &WrapRandomness {
                ephemeral_secret: secret(salt | 0x40),
                seal_nonce: [salt; 32],
                wrap_nonce: [salt.wrapping_add(1); 32],
                seal_created_at: NOW,
                wrap_created_at: NOW,
            },
        )
        .unwrap()
    }

    /// Send `wrap` over the link and wait for the ward's signed ACK to say
    /// `want`, returning how long that took.
    async fn send_expect(&self, wrap: &NostrEvent, want: AckResult) -> Duration {
        let started = Instant::now();
        assert_eq!(
            self.carrier.send(&machine_pk(), wrap.clone()),
            SendState::Sent
        );
        let id = wrap.id;
        let mut last = None;
        let got = eventually(5, || {
            last = self.carrier.delivery(&[id]).get(&id).map(|r| r.state);
            last == Some(DeliveryState::Acked(want))
        })
        .await;
        assert!(got, "expected ACK {want:?} for {id:?}, last state {last:?}");
        started.elapsed()
    }

    /// Send the identical wrap (same id) over the link again. The carrier's
    /// delivery record per wrap id only ever advances, so it keeps the first
    /// ACK; the ward's answer to the repeat is read from the very ingest the
    /// link calls (`BrokerIngest`), on the same broker.
    async fn resend_identical(&self, wrap: &NostrEvent) -> AckResult {
        assert_eq!(
            self.carrier.send(&machine_pk(), wrap.clone()),
            SendState::Sent
        );
        BrokerIngest::new(self.broker.clone(), self.control.clone())
            .ingest(wrap.clone())
            .await
    }

    fn stored_schedule(&self) -> Option<serde_json::Value> {
        self.broker
            .sys()
            .child_clauses()
            .get_child_clause(&SUBJECT.to_hex(), ClauseKind::Schedule.store_key())
            .unwrap()
            .map(|j| serde_json::from_str(&j).unwrap())
    }

    /// The guardian-side inbox, opened with the guardian key: `(inner kind,
    /// inner content)` per wrap the ward offered over the link.
    fn carrier_inbox(&self) -> Vec<(u16, String)> {
        self.carrier
            .inbox()
            .into_iter()
            .map(|e| {
                assert_eq!(e.machine, machine_pk(), "authenticated as this ward");
                let (rumor, author) = nip59::unwrap_with_author(
                    &e.wrap,
                    &secret(GUARDIAN_SEED),
                    NOW,
                    nip59::MAX_JITTER_SECS,
                )
                .expect("a wrap the guardian can open");
                assert_eq!(author, machine_pk(), "sealed by the machine key");
                (rumor.kind, rumor.content)
            })
            .collect()
    }
}

fn time_extend_enactors() -> (EnactorRegistry, ExtensionInbox) {
    let inbox: ExtensionInbox = Arc::default();
    let enforcer = Arc::new(Mutex::new(EnforcerRuntime::fresh("UTC", NOW as i64)));
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(
        TimeExtendEnactor::new(enforcer).with_inbox(inbox.clone()),
    ));
    (reg, inbox)
}

fn schedule_clause(g: &TestGuardian, issued_at: u64, marker: &str) -> NostrEvent {
    ClauseBuilder::schedule(issued_at)
        .subject(SUBJECT)
        .body(json!({ "marker": marker }))
        .build(g)
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clause_over_the_link_applies_and_is_acked_with_every_relay_down() {
    let r = rig("clause", EnactorRegistry::new()).await;
    // The relays really are down for this ward.
    assert!(r.broker.poll_once().await.relays_unreachable);

    let clause = schedule_clause(&r.guardian, 100, "over the home network");
    let wrap = r.wrap(&clause, 1);
    r.send_expect(&wrap, AckResult::Applied).await;
    assert_eq!(
        r.stored_schedule(),
        Some(json!({ "marker": "over the home network" }))
    );

    // The very same wrap again: already held.
    assert_eq!(r.resend_identical(&wrap).await, AckResult::Unchanged);
    // A second wrap of the same clause (a re-sending console): its ACK says
    // so over the wire.
    r.send_expect(&r.wrap(&clause, 9), AckResult::Unchanged)
        .await;
    // An older clause from the guardian: the newer one stays in effect.
    let older = schedule_clause(&r.guardian, 50, "older");
    r.send_expect(&r.wrap(&older, 2), AckResult::Superseded)
        .await;
    assert_eq!(
        r.stored_schedule(),
        Some(json!({ "marker": "over the home network" }))
    );
    // The relays come back with the same wrap: nothing changes.
    r.st.down.store(false, Ordering::SeqCst);
    r.relay.inject(wrap.clone());
    let counts = r.broker.poll_once().await;
    assert_eq!(counts.clauses_accepted, 0, "the relay copy is a duplicate");
    assert_eq!(
        r.stored_schedule(),
        Some(json!({ "marker": "over the home network" }))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_reaches_the_guardian_and_its_time_extend_applies_exactly_once() {
    let (reg, extensions) = time_extend_enactors();
    let r = rig("extend", reg).await;

    // The ward asks for time with every relay down: the ask still reaches the
    // carrier over the link.
    let req = r
        .broker
        .submit(OpType::TimeExtend, json!({ "minutes": 15 }), None)
        .await
        .unwrap();
    assert!(
        eventually(5, || r
            .carrier_inbox()
            .iter()
            .any(|(k, _)| *k == kinds::CHARTER_DEVICE_REQUEST))
        .await,
        "the REQUEST reached the guardian with no relay"
    );
    let (_, content) = r
        .carrier_inbox()
        .into_iter()
        .find(|(k, _)| *k == kinds::CHARTER_DEVICE_REQUEST)
        .unwrap();
    assert!(content.contains(&req.to_hex()), "it is this ask");
    // It reached the guardian, so it left the ward's outbox.
    assert!(eventually(5, || r.ward.snapshot().outbox == 0).await);

    // The guardian answers over the link.
    let rec = r.broker.status(&req.to_hex()).pop().unwrap();
    assert_eq!(rec.state, RequestState::Pending);
    let grant = GrantBuilder::install_allow(rec.req_id, rec.nonce)
        .op(OpType::TimeExtend)
        .params(json!({ "minutesGranted": 15, "limitHit": "budget" }))
        .build(&r.guardian);
    let wrap = r.wrap(&grant, 1);
    r.send_expect(&wrap, AckResult::Applied).await;
    assert_eq!(
        r.broker.status(&req.to_hex()).pop().unwrap().state,
        RequestState::Enacted
    );
    assert_eq!(extensions.lock().unwrap().len(), 1, "extended once");

    // The same wrap over the link again, then the relays return it too.
    assert_eq!(r.resend_identical(&wrap).await, AckResult::Unchanged);
    r.st.down.store(false, Ordering::SeqCst);
    r.relay.inject(wrap.clone());
    r.broker.poll_once().await;
    // And a second wrap of the same grant, as a re-sending console makes.
    r.send_expect(&r.wrap(&grant, 2), AckResult::Unchanged)
        .await;
    assert_eq!(
        extensions.lock().unwrap().len(),
        1,
        "still exactly one extension"
    );
    assert_eq!(
        r.broker.status(&req.to_hex()).pop().unwrap().state,
        RequestState::Enacted
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stuck_relay_poll_does_not_hold_up_a_change_over_the_link() {
    let r = rig("stuck", EnactorRegistry::new()).await;
    r.st.down.store(false, Ordering::SeqCst);
    r.st.stuck.store(true, Ordering::SeqCst);
    let mut poller =
        BrokerPoller::spawn(r.broker.clone(), r.control.clone(), Duration::from_secs(10));
    assert!(
        eventually(5, || poller.in_flight_for().is_some()
            && r.st.queries.load(Ordering::SeqCst) > 0)
        .await,
        "a poll round is in flight, waiting on the relay"
    );

    let clause = schedule_clause(&r.guardian, 100, "while the relay hangs");
    let took = r.send_expect(&r.wrap(&clause, 1), AckResult::Applied).await;
    assert!(
        took < Duration::from_secs(2),
        "applied within one fast tick, took {took:?}"
    );
    assert_eq!(
        r.stored_schedule(),
        Some(json!({ "marker": "while the relay hangs" }))
    );
    assert!(poller.in_flight_for().is_some(), "the poll is still stuck");
    assert!(poller.drain().is_empty());

    // The relay recovers: the round finishes and is reported.
    r.st.stuck.store(false, Ordering::SeqCst);
    let mut rounds = Vec::new();
    assert!(
        eventually(5, || {
            rounds.extend(poller.drain());
            !rounds.is_empty()
        })
        .await
    );
    assert!(!rounds[0].relays_unreachable);
    assert!(!rounds[0].released);
    poller.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_over_the_link_reaches_the_loop_and_stops_the_poll() {
    let r = rig("release", EnactorRegistry::new()).await;
    r.broker
        .sys()
        .pairing()
        .save(
            &json!({
                "guardian_pubkey": r.guardian.pubkey().to_hex(),
                "relays": [RELAY],
                "machine": machine_pk().to_hex(),
                "subject_pubkey": SUBJECT.to_hex(),
                "audit_transparency": false,
                "paired_at": NOW - 100,
            })
            .to_string(),
        )
        .unwrap();
    r.send_expect(
        &r.wrap(&schedule_clause(&r.guardian, 100, "before"), 1),
        AckResult::Applied,
    )
    .await;

    let release = sign_event(
        &r.guardian.signer,
        kinds::CHARTER_DEVICE_RELEASE,
        NOW,
        vec![kinds::marker_tag()],
        json!({ "v": 1, "machine": machine_pk().to_hex(), "issuedAt": NOW }).to_string(),
    );
    assert!(!r.control.released());
    r.send_expect(&r.wrap(&release, 2), AckResult::Applied)
        .await;
    assert!(r.control.released(), "the loop is told, and restarts");
    assert!(r.broker.sys().pairing().load().unwrap().is_none());
    assert_eq!(r.stored_schedule(), None);

    // Nothing lands afterwards on either route.
    r.send_expect(
        &r.wrap(&schedule_clause(&r.guardian, 200, "after"), 3),
        AckResult::Refused,
    )
    .await;
    assert_eq!(r.stored_schedule(), None);
    let mut poller = BrokerPoller::spawn(
        r.broker.clone(),
        r.control.clone(),
        Duration::from_millis(10),
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(poller.drain().is_empty(), "no poll round after a release");
    assert!(poller.in_flight_for().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_poll_round_tells_the_loop_too() {
    let r = rig("release-poll", EnactorRegistry::new()).await;
    r.broker
        .sys()
        .pairing()
        .save(
            &json!({
                "guardian_pubkey": r.guardian.pubkey().to_hex(),
                "relays": [RELAY],
                "machine": machine_pk().to_hex(),
                "subject_pubkey": SUBJECT.to_hex(),
                "audit_transparency": false,
                "paired_at": NOW - 100,
            })
            .to_string(),
        )
        .unwrap();
    let release = sign_event(
        &r.guardian.signer,
        kinds::CHARTER_DEVICE_RELEASE,
        NOW,
        vec![kinds::marker_tag()],
        json!({ "v": 1, "machine": machine_pk().to_hex(), "issuedAt": NOW }).to_string(),
    );
    r.st.down.store(false, Ordering::SeqCst);
    r.relay.inject(r.wrap(&release, 1));
    let mut poller = BrokerPoller::spawn(
        r.broker.clone(),
        r.control.clone(),
        Duration::from_millis(10),
    );
    let mut rounds = Vec::new();
    assert!(
        eventually(5, || {
            rounds.extend(poller.drain());
            !rounds.is_empty()
        })
        .await
    );
    assert!(rounds[0].released);
    assert!(r.control.released());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(poller.drain().is_empty(), "the poll stops after a release");
}
