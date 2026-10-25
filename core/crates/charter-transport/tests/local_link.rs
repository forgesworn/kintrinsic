//! The transport's two local-link seams: `unwrap_inbound` opens a wrap that
//! arrived outside relay polling through the same code as `poll_*`, and an
//! attached `LocalSink` is offered every machine→guardian wrap before any
//! relay IO, without changing what the relays see or report.

#![cfg(feature = "mock")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use charter_link::{LocalOffer, LocalSink};
use charter_primitives::{kinds, Nonce, NostrEvent, PubKey, ReqId};
use charter_sys::relay::{
    Filter, MockRelayTransport, PublishOutcome, RelayIoError, RelayTransport, RelayUrl,
};
use charter_sys::signer::SeedSigner;
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, GrantBuilder, TestGuardian};

const NOW: u64 = 1_700_001_000;

fn secret(b: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = b;
    s
}

fn pk_of(sk: &[u8; 32]) -> PubKey {
    PubKey::from_bytes(charter_crypto::xonly_pubkey(sk).unwrap())
}

fn machine_sk() -> [u8; 32] {
    secret(0x22)
}

/// The guardian's seal key (the broker ignores the seal author; the inner
/// event's signature is what is authenticated downstream).
fn seal_sk() -> [u8; 32] {
    secret(0x11)
}

fn randomness(salt: u8) -> WrapRandomness {
    WrapRandomness {
        ephemeral_secret: secret(salt | 0x40),
        seal_nonce: [salt; 32],
        wrap_nonce: [salt.wrapping_add(1); 32],
        seal_created_at: NOW,
        wrap_created_at: NOW,
    }
}

/// Wrap `ev` to `recipient` as the guardian would.
fn wrap_to(ev: &NostrEvent, recipient: &PubKey, salt: u8) -> NostrEvent {
    nip59::wrap(
        &Rumor::from_signed_event(ev),
        &seal_sk(),
        recipient.as_bytes(),
        &randomness(salt),
    )
    .unwrap()
}

/// Re-sign `wrap` with its own ephemeral key under different tags: still a
/// validly signed wrap that decrypts for its real recipient.
fn retag(wrap: &NostrEvent, salt: u8, tags: Vec<Vec<String>>) -> NostrEvent {
    sign_event(
        &SeedSigner::from_secret(secret(salt | 0x40)),
        kinds::GIFT_WRAP,
        wrap.created_at,
        tags,
        wrap.content.clone(),
    )
}

fn transport<R: RelayTransport>(relay: R) -> CharterTransport<R, ScriptedEntropy> {
    CharterTransport::new(
        relay,
        ScriptedEntropy::new(7),
        machine_sk(),
        TestGuardian::new().pubkey(),
        vec!["wss://r".into(), "wss://down".into()],
    )
}

#[test]
fn unwrap_inbound_opens_what_polling_opens() {
    let guardian = TestGuardian::new();
    let t = transport(MockRelayTransport::new());
    let clause = ClauseBuilder::schedule(100).build(&guardian);
    let w = wrap_to(&clause, &pk_of(&machine_sk()), 0x01);
    let (inner, author) = t.unwrap_inbound(&w, NOW).expect("opens");
    assert_eq!(inner, clause, "the inner event, byte for byte");
    assert_eq!(author, pk_of(&seal_sk()));
}

#[tokio::test]
async fn unwrap_inbound_and_poll_agree_on_every_wrap() {
    let guardian = TestGuardian::new();
    let relay = MockRelayTransport::new();
    let t = transport(relay.clone());
    let grant = GrantBuilder::install_allow(ReqId::from_bytes([1; 32]), Nonce::from_bytes([2; 32]))
        .build(&guardian);
    let w = wrap_to(&grant, &pk_of(&machine_sk()), 0x03);
    relay.inject(w.clone());
    let polled = t.poll_grants(0, NOW).await.unwrap();
    assert_eq!(polled.len(), 1);
    let (local, _) = t.unwrap_inbound(&w, NOW).unwrap();
    assert_eq!(polled[0].grant, local);
}

#[test]
fn unwrap_inbound_refuses_what_is_not_for_this_machine() {
    let guardian = TestGuardian::new();
    let t = transport(MockRelayTransport::new());
    let clause = ClauseBuilder::schedule(100).build(&guardian);
    let me = pk_of(&machine_sk());

    // Encrypted to someone else entirely.
    let other = wrap_to(&clause, &pk_of(&secret(0x33)), 0x05);
    assert!(t.unwrap_inbound(&other, NOW).is_none());

    // Encrypted to this machine and validly signed, but p-tagged elsewhere,
    // or carrying a second p tag: the relay's #p filter would have decided
    // these, so the local path checks the tag itself.
    let w = wrap_to(&clause, &me, 0x06);
    let control = retag(&w, 0x06, vec![vec!["p".into(), me.to_hex()]]);
    assert!(
        t.unwrap_inbound(&control, NOW).is_some(),
        "control: re-signing alone does not break the wrap"
    );
    let elsewhere = retag(
        &w,
        0x06,
        vec![vec!["p".into(), pk_of(&secret(0x33)).to_hex()]],
    );
    assert!(t.unwrap_inbound(&elsewhere, NOW).is_none());
    let two = retag(
        &w,
        0x06,
        vec![
            vec!["p".into(), me.to_hex()],
            vec!["p".into(), pk_of(&secret(0x33)).to_hex()],
        ],
    );
    assert!(t.unwrap_inbound(&two, NOW).is_none());
    let none = retag(&w, 0x06, vec![]);
    assert!(t.unwrap_inbound(&none, NOW).is_none());

    // Tampered: the outer signature no longer verifies.
    let mut tampered = w.clone();
    tampered.content.push('A');
    assert!(t.unwrap_inbound(&tampered, NOW).is_none());

    // Outside the transport's jitter window, exactly as on the poll path.
    assert!(t
        .unwrap_inbound(&w, NOW + nip59::MAX_JITTER_SECS + 1)
        .is_none());

    // A bare rumor (no inner id/sig) does not come out as an event.
    let bare = Rumor {
        id: None,
        pubkey: guardian.pubkey(),
        created_at: NOW,
        kind: kinds::CHARTER_DEVICE_CLAUSE,
        tags: vec![],
        content: "{}".into(),
        sig: None,
    };
    let bare_wrap = nip59::wrap(&bare, &seal_sk(), me.as_bytes(), &randomness(0x07)).unwrap();
    assert!(t.unwrap_inbound(&bare_wrap, NOW).is_none());
}

/// One timeline shared by the sink and the relay, so ordering is observable.
type Log = Arc<Mutex<Vec<String>>>;

struct RecordingSink {
    log: Log,
    offers: Mutex<Vec<LocalOffer>>,
}

impl LocalSink for RecordingSink {
    fn offer(&self, offer: LocalOffer) {
        self.log
            .lock()
            .unwrap()
            .push(format!("offer {}", offer.wrap.id.to_hex()));
        self.offers.lock().unwrap().push(offer);
    }
}

#[derive(Clone)]
struct RecordingRelay {
    inner: MockRelayTransport,
    log: Log,
}

#[async_trait]
impl RelayTransport for RecordingRelay {
    async fn publish(
        &self,
        relays: &[RelayUrl],
        ev: NostrEvent,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        self.log
            .lock()
            .unwrap()
            .push(format!("publish {}", ev.id.to_hex()));
        self.inner.publish(relays, ev).await
    }

    async fn query(
        &self,
        relays: &[RelayUrl],
        filter: Filter,
    ) -> Result<Vec<NostrEvent>, RelayIoError> {
        self.inner.query(relays, filter).await
    }
}

fn status_json(subject: &PubKey) -> String {
    format!(r#"{{"v":1,"subject":"{}","ts":{NOW}}}"#, subject.to_hex())
}

#[tokio::test]
async fn every_machine_wrap_is_offered_locally_before_relay_io_with_the_same_id() {
    let log: Log = Arc::default();
    let relay = RecordingRelay {
        inner: MockRelayTransport::new().with_failing_relay("wss://down"),
        log: log.clone(),
    };
    let sink = Arc::new(RecordingSink {
        log: log.clone(),
        offers: Mutex::default(),
    });
    let t = transport(relay).with_local_sink(sink.clone());
    let subject = PubKey::from_bytes([0xA1; 32]);

    t.submit_request(r#"{"v":1}"#, NOW).await;
    t.emit_status(&status_json(&subject), NOW).await;
    t.emit_audit(vec![vec!["outcome".into(), "enacted".into()]], NOW)
        .await;
    // The spool shape: build, then publish the prebuilt wrap.
    let req = t.build_request_wrap(r#"{"v":1,"n":2}"#, NOW).unwrap();
    t.publish_prebuilt(req.clone()).await;
    let st = t.build_status_wrap(&status_json(&subject), NOW).unwrap();
    t.publish_prebuilt(st.clone()).await;
    // A parked wrap re-published later is NOT offered again.
    t.publish_prebuilt(req.clone()).await;

    let offers = sink.offers.lock().unwrap().clone();
    let kinds_seen: Vec<u16> = offers.iter().map(|o| o.inner_kind).collect();
    assert_eq!(
        kinds_seen,
        vec![
            kinds::CHARTER_DEVICE_REQUEST,
            kinds::CHARTER_DEVICE_STATUS,
            kinds::CHARTER_DEVICE_AUDIT,
            kinds::CHARTER_DEVICE_REQUEST,
            kinds::CHARTER_DEVICE_STATUS,
        ]
    );
    let status_key = Some(format!("status:{}", subject.to_hex()));
    for o in &offers {
        let want = (o.inner_kind == kinds::CHARTER_DEVICE_STATUS).then(|| status_key.clone());
        assert_eq!(o.key, want.flatten(), "only STATUS carries a key");
        assert_eq!(o.wrap.kind, kinds::GIFT_WRAP);
    }
    assert_eq!(
        offers[3].wrap, req,
        "the spooled REQUEST is the wrap offered"
    );
    assert_eq!(offers[4].wrap, st);

    // Each wrap: offered, THEN published, under one event id.
    let log = log.lock().unwrap().clone();
    let mut expected = Vec::new();
    for o in &offers {
        let id = o.wrap.id.to_hex();
        expected.push(format!("offer {id}"));
        expected.push(format!("publish {id}"));
    }
    expected.push(format!("publish {}", req.id.to_hex()));
    assert_eq!(log, expected);
}

#[tokio::test]
async fn a_sink_changes_no_relay_outcome() {
    let subject = PubKey::from_bytes([0xA1; 32]);
    let run = |with_sink: bool| async move {
        let relay = MockRelayTransport::new().with_failing_relay("wss://down");
        let mut t = transport(relay.clone());
        if with_sink {
            t.set_local_sink(Arc::new(RecordingSink {
                log: Arc::default(),
                offers: Mutex::default(),
            }));
        }
        let outcomes = vec![
            t.submit_request(r#"{"v":1}"#, NOW).await,
            t.emit_status(&status_json(&subject), NOW).await,
            t.emit_audit(vec![], NOW).await,
        ];
        (outcomes, relay.len())
    };
    let (without, n_without) = run(false).await;
    let (with, n_with) = run(true).await;
    assert_eq!(with, without);
    assert_eq!(n_with, n_without);
    assert_eq!(n_with, 3);
    assert!(matches!(without[0][1].1, PublishOutcome::Failed(_)));
}

#[tokio::test]
async fn a_pair_offer_is_never_offered_locally() {
    // Guardian side: the transport addresses an unpaired ward.
    let guardian_sk = secret(0x11);
    let ward = pk_of(&machine_sk());
    let sink = Arc::new(RecordingSink {
        log: Arc::default(),
        offers: Mutex::default(),
    });
    let t = CharterTransport::new(
        MockRelayTransport::new(),
        ScriptedEntropy::new(9),
        guardian_sk,
        ward,
        vec!["wss://r".into()],
    )
    .with_local_sink(sink.clone());
    t.send_pair_offer("token", NOW).await;
    assert!(sink.offers.lock().unwrap().is_empty());
}
