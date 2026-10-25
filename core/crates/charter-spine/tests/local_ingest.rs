//! `Broker::ingest_local`: a guardian wrap delivered over the local link goes
//! through the same unwrap, verification and state as its relay copy, and the
//! result it reports (for the ward's signed ACK) says truthfully what this
//! delivery did. Runs the real `CharterTransport` (NIP-59, NIP-44) over the
//! mock relay and the mock system, so the relay copy is a real `poll_once`.

#![cfg(feature = "mock")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use charter_link::{AckResult, LocalOffer, LocalSink};
use charter_primitives::{kinds, Nonce, NostrEvent, PubKey, ReqId};
use charter_proto::{ClauseKind, OpType};
use charter_spine::enactor::{EnactContext, EnactOutcome, Enactor};
use charter_spine::enactors::time_extend::TimeExtendEnactor;
use charter_spine::enforcer_runtime::EnforcerRuntime;
use charter_spine::multi_child::ExtensionInbox;
use charter_spine::ports::NullEventSink;
use charter_spine::{
    Broker, CharterTransportAdapter, EnactError, EnactorRegistry, LocalIngestOptions,
    RequestRecord, RequestState,
};
use charter_sys::persistence::{ChildClauseStore, ConsumedIdStore, MockDisk, PairingStore};
use charter_sys::relay::{Filter, MockRelayTransport, RelayTransport};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, GrantBuilder, TestGuardian};
use charter_verify::VerifiedGrant;
use serde_json::json;

const NOW: u64 = 1_700_001_000;
/// The grant builder's default validity window (`ts`, `exp`).
const GRANT_EXP: u64 = 1_700_003_600;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);
const ALICE: PubKey = PubKey::from_bytes([0xA1; 32]);
/// `TestGuardian::new()` is the key with low byte 0x11; a stranger is 0x33.
const GUARDIAN_SEED: u8 = 0x11;
const STRANGER_SEED: u8 = 0x33;

type Facade = CharterTransportAdapter<MockRelayTransport, ScriptedEntropy>;
type B = Broker<MockSystem, Facade, ScriptedEntropy>;

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

const LOCAL: LocalIngestOptions = LocalIngestOptions {
    handle_release: true,
};

/// A ward's broker over `disk` and `relay`: what survives a restart is the
/// disk; the relay is the outside world.
struct Ward {
    guardian: TestGuardian,
    disk: MockDisk,
    relay: MockRelayTransport,
}

impl Ward {
    fn new() -> Self {
        Ward {
            guardian: TestGuardian::new(),
            disk: MockDisk::new(),
            relay: MockRelayTransport::new(),
        }
    }

    fn broker_at(
        &self,
        now: u64,
        enactors: EnactorRegistry,
        sink: Option<Arc<dyn LocalSink>>,
    ) -> B {
        let mut t = CharterTransport::new(
            self.relay.clone(),
            ScriptedEntropy::new(7),
            machine_sk(),
            self.guardian.pubkey(),
            vec!["wss://r".into()],
        );
        if let Some(sink) = sink {
            t.set_local_sink(sink);
        }
        Broker::new(
            MockSystem::over_disk(now, self.disk.clone()),
            CharterTransportAdapter::new(t),
            // Distinct per boot only matters for ids generated after it.
            ScriptedEntropy::new(1),
            enactors,
            Box::new(NullEventSink),
            SUBJECT,
        )
    }

    fn broker(&self) -> B {
        self.broker_at(NOW, EnactorRegistry::new(), None)
    }

    /// The guardian gift-wraps `ev` to this ward. A different `salt` is a
    /// different wrap (new ephemeral key, new id) of the same inner event.
    fn wrap(&self, ev: &NostrEvent, salt: u8) -> NostrEvent {
        self.wrap_at(ev, salt, NOW)
    }

    fn wrap_at(&self, ev: &NostrEvent, salt: u8, at: u64) -> NostrEvent {
        self.sealed(ev, GUARDIAN_SEED, salt, at)
    }

    /// `ev` sealed by the key with low byte `seed` (NIP-59 requires the seal
    /// author to be the inner author, so a stranger's forgery is sealed by
    /// the stranger and reaches verification).
    fn sealed(&self, ev: &NostrEvent, seed: u8, salt: u8, at: u64) -> NostrEvent {
        let r = WrapRandomness {
            ephemeral_secret: secret(salt | 0x40),
            seal_nonce: [salt; 32],
            wrap_nonce: [salt.wrapping_add(1); 32],
            seal_created_at: at,
            wrap_created_at: at,
        };
        nip59::wrap(
            &Rumor::from_signed_event(ev),
            &secret(seed),
            machine_pk().as_bytes(),
            &r,
        )
        .unwrap()
    }
}

fn stored_child_clause(b: &B, subject: &PubKey, kind: ClauseKind) -> Option<serde_json::Value> {
    b.sys()
        .child_clauses()
        .get_child_clause(&subject.to_hex(), kind.store_key())
        .unwrap()
        .map(|j| serde_json::from_str(&j).unwrap())
}

fn only_request(b: &B) -> RequestRecord {
    let all = b.status("");
    assert_eq!(all.len(), 1, "one request");
    all[0].clone()
}

fn record(b: &B, req: &ReqId) -> RequestRecord {
    b.status(&req.to_hex()).pop().expect("record")
}

// ---------------------------------------------------------------------------
// Clauses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_local_clause_applies_once_and_every_later_copy_is_unchanged() {
    let w = Ward::new();
    let b = w.broker();
    let clause = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .body(json!({"marker": "first"}))
        .build(&w.guardian);
    let wrap = w.wrap(&clause, 1);

    let out = b.ingest_local(&wrap, LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(out.inner_kind, Some(kinds::CHARTER_DEVICE_CLAUSE));
    assert_eq!(out.inner_id, Some(clause.id));
    assert_eq!(out.req_id, None);
    assert!(!out.released && out.release.is_none());
    assert_eq!(
        stored_child_clause(&b, &ALICE, ClauseKind::Schedule),
        Some(json!({"marker": "first"}))
    );

    // The same wrap again, and a different wrap of the same clause.
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
    assert_eq!(
        b.ingest_local(&w.wrap(&clause, 2), LOCAL).await.result,
        AckResult::Unchanged
    );

    // The relay copy (the identical wrap) is seen, refused as not newer, and
    // changes nothing.
    w.relay.inject(wrap.clone());
    let counts = b.poll_once().await;
    assert_eq!(counts.clauses_seen, 1);
    assert_eq!(counts.clauses_accepted, 0);
    assert_eq!(counts.clauses_write_failed, 0);
    assert_eq!(
        stored_child_clause(&b, &ALICE, ClauseKind::Schedule),
        Some(json!({"marker": "first"}))
    );

    // ... and after a restart the local copy is still recognised.
    let b = w.broker();
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
}

#[tokio::test]
async fn an_older_relay_clause_never_rolls_back_a_newer_local_one() {
    let w = Ward::new();
    let b = w.broker();
    let newer = ClauseBuilder::budget(200)
        .subject(ALICE)
        .body(json!({"marker": "newer"}))
        .build(&w.guardian);
    let older = ClauseBuilder::budget(100)
        .subject(ALICE)
        .body(json!({"marker": "older"}))
        .build(&w.guardian);

    assert_eq!(
        b.ingest_local(&w.wrap(&newer, 1), LOCAL).await.result,
        AckResult::Applied
    );
    let older_wrap = w.wrap(&older, 2);
    w.relay.inject(older_wrap.clone());
    let counts = b.poll_once().await;
    assert_eq!((counts.clauses_seen, counts.clauses_accepted), (1, 0));
    assert_eq!(
        stored_child_clause(&b, &ALICE, ClauseKind::Budget),
        Some(json!({"marker": "newer"}))
    );

    // Restart over the same stores: the relay keeps re-offering the older
    // clause inside the lookback, and it is still refused.
    let b = w.broker();
    let counts = b.poll_once().await;
    assert_eq!((counts.clauses_seen, counts.clauses_accepted), (1, 0));
    assert_eq!(
        b.ingest_local(&older_wrap, LOCAL).await.result,
        AckResult::Superseded,
        "authentic but older: the newer clause stays in effect"
    );
    assert_eq!(
        stored_child_clause(&b, &ALICE, ClauseKind::Budget),
        Some(json!({"marker": "newer"}))
    );
}

#[tokio::test]
async fn a_tied_issued_at_with_different_content_is_superseded() {
    let w = Ward::new();
    let b = w.broker();
    let a = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .body(json!({"marker": "a"}))
        .build(&w.guardian);
    let other = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .body(json!({"marker": "b"}))
        .build(&w.guardian);
    assert_ne!(a.id, other.id);

    assert_eq!(
        b.ingest_local(&w.wrap(&a, 1), LOCAL).await.result,
        AckResult::Applied
    );
    assert_eq!(
        b.ingest_local(&w.wrap(&other, 2), LOCAL).await.result,
        AckResult::Superseded
    );
    assert_eq!(
        stored_child_clause(&b, &ALICE, ClauseKind::Schedule),
        Some(json!({"marker": "a"}))
    );
}

#[tokio::test]
async fn a_resend_at_a_sidecar_floor_is_applied_and_then_unchanged() {
    let w = Ward::new();
    let b = w.broker();
    let first = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .build(&w.guardian);
    assert_eq!(
        b.ingest_local(&w.wrap(&first, 1), LOCAL).await.result,
        AckResult::Applied
    );
    // The record is torn; the floor now comes from the sidecar.
    b.sys().disk().tear_clause_records();
    let older = ClauseBuilder::schedule(99)
        .subject(ALICE)
        .build(&w.guardian);
    assert_eq!(
        b.ingest_local(&w.wrap(&older, 2), LOCAL).await.result,
        AckResult::Superseded,
        "below the floor is still a replay"
    );
    // The guardian's re-send at exactly the sidecar floor is taken (R2-7),
    // and once the record is mended a repeat is just a duplicate.
    let resend = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .body(json!({"resent": true}))
        .build(&w.guardian);
    let resend_wrap = w.wrap(&resend, 3);
    assert_eq!(
        b.ingest_local(&resend_wrap, LOCAL).await.result,
        AckResult::Applied
    );
    assert_eq!(
        b.ingest_local(&resend_wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
}

#[tokio::test]
async fn a_machine_wide_clause_reports_the_same_way() {
    let w = Ward::new();
    let b = w.broker();
    let c = ClauseBuilder::content(100).build(&w.guardian);
    let wrap = w.wrap(&c, 1);
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Applied
    );
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
}

#[tokio::test]
async fn a_forged_clause_is_refused_and_nothing_is_stored() {
    let w = Ward::new();
    let b = w.broker();
    let stranger = TestGuardian::from_seed(STRANGER_SEED);
    let forged = ClauseBuilder::schedule(100).subject(ALICE).build(&stranger);
    let out = b
        .ingest_local(&w.sealed(&forged, STRANGER_SEED, 1, NOW), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(out.inner_id, Some(forged.id));
    assert_eq!(stored_child_clause(&b, &ALICE, ClauseKind::Schedule), None);

    // The genuine clause at the same issuedAt still lands: the forgery moved
    // no floor.
    let genuine = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .build(&w.guardian);
    assert_eq!(
        b.ingest_local(&w.wrap(&genuine, 2), LOCAL).await.result,
        AckResult::Applied
    );
}

#[tokio::test]
async fn a_clause_whose_write_fails_is_failed_not_applied() {
    let w = Ward::new();
    let b = w.broker();
    b.sys().disk().break_clause_writes();
    let c = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .build(&w.guardian);
    assert_eq!(
        b.ingest_local(&w.wrap(&c, 1), LOCAL).await.result,
        AckResult::Failed
    );
}

#[tokio::test]
async fn junk_and_foreign_wraps_are_refused() {
    let w = Ward::new();
    let b = w.broker();
    let c = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .build(&w.guardian);
    let mut tampered = w.wrap(&c, 1);
    tampered.content.push('A');
    let out = b.ingest_local(&tampered, LOCAL).await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(out.inner_kind, None);
    assert_eq!(out.inner_id, None);
}

#[tokio::test]
async fn a_kind_not_acted_on_locally_is_ignored() {
    let w = Ward::new();
    let b = w.broker();
    let ev = sign_event(
        &w.guardian.signer,
        kinds::CHARTER_DEVICE_RELAYS,
        NOW,
        vec![kinds::marker_tag()],
        "{}".into(),
    );
    let out = b.ingest_local(&w.wrap(&ev, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Ignored);
    assert_eq!(out.inner_kind, Some(kinds::CHARTER_DEVICE_RELAYS));
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

fn time_extend_enactors() -> (EnactorRegistry, ExtensionInbox) {
    let inbox: ExtensionInbox = Arc::default();
    let enforcer = Arc::new(Mutex::new(EnforcerRuntime::fresh("UTC", NOW as i64)));
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(
        TimeExtendEnactor::new(enforcer).with_inbox(inbox.clone()),
    ));
    (reg, inbox)
}

fn extend_grant(w: &Ward, rec: &RequestRecord) -> NostrEvent {
    GrantBuilder::install_allow(rec.req_id, rec.nonce)
        .op(OpType::TimeExtend)
        .params(json!({"minutesGranted": 15, "limitHit": "budget"}))
        .build(&w.guardian)
}

#[tokio::test]
async fn a_local_time_extend_extends_once_whatever_else_arrives() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({"minutes": 15}), None)
        .await
        .unwrap();
    let rec = only_request(&b);
    let grant = extend_grant(&w, &rec);
    let wrap = w.wrap(&grant, 1);

    let out = b.ingest_local(&wrap, LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(out.req_id, Some(rec.req_id));
    assert_eq!(out.inner_id, Some(grant.id));
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Enacted);
    assert_eq!(inbox.lock().unwrap().len(), 1);

    // The relay copy, and a different wrap of the same grant locally.
    w.relay.inject(wrap.clone());
    b.poll_once().await;
    assert_eq!(
        b.ingest_local(&w.wrap(&grant, 2), LOCAL).await.result,
        AckResult::Unchanged
    );
    assert_eq!(inbox.lock().unwrap().len(), 1, "one extension only");

    // Restart: the request is Enacted on disk and its reqId consumed.
    let (reg2, inbox2) = time_extend_enactors();
    let b = w.broker_at(NOW, reg2, None);
    b.poll_once().await;
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
    assert!(
        inbox2.lock().unwrap().is_empty(),
        "nothing re-enacted after restart"
    );
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Enacted);
}

#[tokio::test]
async fn a_relay_grant_first_makes_the_local_copy_unchanged() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    let wrap = w.wrap(&extend_grant(&w, &rec), 1);
    w.relay.inject(wrap.clone());
    b.poll_once().await;
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Enacted);
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
    assert_eq!(inbox.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_deny_is_applied() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    let deny = GrantBuilder::install_allow(rec.req_id, rec.nonce)
        .op(OpType::TimeExtend)
        .deny()
        .build(&w.guardian);
    assert_eq!(
        b.ingest_local(&w.wrap(&deny, 1), LOCAL).await.result,
        AckResult::Applied
    );
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Denied);
    assert!(inbox.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_grant_past_its_window_is_expired_and_never_enacted() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    let grant = extend_grant(&w, &rec);
    // Past exp + the freshness skew, well inside the wrap jitter window.
    let late = GRANT_EXP + 301;
    b.sys().mock_clock().set_utc(late);
    let out = b.ingest_local(&w.wrap_at(&grant, 1, late), LOCAL).await;
    assert_eq!(out.result, AckResult::Expired);
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Pending);
    assert!(inbox.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_grant_for_an_ask_that_aged_out_is_expired() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    // A day and a bit later: the ask is past its TTL, and the guardian's
    // (otherwise fresh) answer arrives over the link before any poll ran.
    let later = NOW + 25 * 3600;
    b.sys().mock_clock().set_utc(later);
    let grant = GrantBuilder::install_allow(rec.req_id, rec.nonce)
        .op(OpType::TimeExtend)
        .params(json!({"minutesGranted": 15, "limitHit": "budget"}))
        .ts(later - 10)
        .exp(later + 600)
        .build(&w.guardian);
    let out = b.ingest_local(&w.wrap_at(&grant, 1, later), LOCAL).await;
    assert_eq!(out.result, AckResult::Expired);
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Expired);
    assert!(inbox.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_forged_grant_is_refused_and_the_genuine_one_still_applies() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    let stranger = TestGuardian::from_seed(STRANGER_SEED);
    let forged = GrantBuilder::install_allow(rec.req_id, rec.nonce)
        .op(OpType::TimeExtend)
        .params(json!({"minutesGranted": 600, "limitHit": "budget"}))
        .build(&stranger);
    let out = b
        .ingest_local(&w.sealed(&forged, STRANGER_SEED, 1, NOW), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(
        out.inner_id,
        Some(forged.id),
        "it opened, and verification refused it"
    );
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Pending);
    assert!(inbox.lock().unwrap().is_empty());

    assert_eq!(
        b.ingest_local(&w.wrap(&extend_grant(&w, &rec), 2), LOCAL)
            .await
            .result,
        AckResult::Applied,
        "a forgery consumes nothing"
    );
}

#[tokio::test]
async fn a_consumed_reqid_on_a_still_pending_ask_is_failed_not_unchanged() {
    let w = Ward::new();
    let (reg, inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, None);
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    // An earlier copy was consumed but its progress never recorded.
    assert!(b
        .sys()
        .consumed_ids()
        .check_and_consume(&rec.req_id, GRANT_EXP + 300)
        .unwrap());
    let out = b
        .ingest_local(&w.wrap(&extend_grant(&w, &rec), 1), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Failed);
    assert_eq!(record(&b, &rec.req_id).state, RequestState::Pending);
    assert!(inbox.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_grant_for_no_known_request_is_refused() {
    let w = Ward::new();
    let b = w.broker();
    let grant = GrantBuilder::install_allow(ReqId::from_bytes([9; 32]), Nonce::from_bytes([8; 32]))
        .build(&w.guardian);
    let out = b.ingest_local(&w.wrap(&grant, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(out.req_id, Some(ReqId::from_bytes([9; 32])));
}

// ---------------------------------------------------------------------------
// Releases and usage syncs
// ---------------------------------------------------------------------------

fn pair(b: &B, guardian: &TestGuardian) {
    b.sys()
        .pairing()
        .save(
            &json!({
                "guardian_pubkey": guardian.pubkey().to_hex(),
                "relays": ["wss://r"],
                "machine": machine_pk().to_hex(),
                "subject_pubkey": SUBJECT.to_hex(),
                "audit_transparency": false,
                "paired_at": NOW - 100,
            })
            .to_string(),
        )
        .unwrap();
}

fn release_event(g: &TestGuardian, issued_at: u64) -> NostrEvent {
    sign_event(
        &g.signer,
        kinds::CHARTER_DEVICE_RELEASE,
        issued_at,
        vec![kinds::marker_tag()],
        json!({"v": 1, "machine": machine_pk().to_hex(), "issuedAt": issued_at}).to_string(),
    )
}

fn paired(b: &B) -> bool {
    b.sys().pairing().load().unwrap().is_some()
}

#[tokio::test]
async fn a_release_is_applied_here_when_asked_to() {
    let w = Ward::new();
    let b = w.broker();
    pair(&b, &w.guardian);
    let ev = release_event(&w.guardian, NOW);
    let out = b
        .ingest_local(
            &w.wrap(&ev, 1),
            LocalIngestOptions {
                handle_release: true,
            },
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(out.released);
    assert!(out.release.is_none());
    assert_eq!(out.inner_kind, Some(kinds::CHARTER_DEVICE_RELEASE));
    assert!(!paired(&b));
}

#[tokio::test]
async fn a_release_is_handed_back_untouched_otherwise() {
    let w = Ward::new();
    let b = w.broker();
    pair(&b, &w.guardian);
    let c = ClauseBuilder::schedule(100)
        .subject(SUBJECT)
        .build(&w.guardian);
    b.ingest_local(&w.wrap(&c, 2), LOCAL).await;
    let ev = release_event(&w.guardian, NOW);
    let out = b
        .ingest_local(
            &w.wrap(&ev, 1),
            LocalIngestOptions {
                handle_release: false,
            },
        )
        .await;
    assert_eq!(out.result, AckResult::Ignored);
    assert!(!out.released);
    assert_eq!(
        out.release,
        Some((ev, w.guardian.pubkey())),
        "with its seal author"
    );
    assert!(paired(&b), "nothing applied");
    assert!(stored_child_clause(&b, &SUBJECT, ClauseKind::Schedule).is_some());
}

#[tokio::test]
async fn a_forged_release_is_refused() {
    let w = Ward::new();
    let b = w.broker();
    pair(&b, &w.guardian);
    let ev = release_event(&TestGuardian::from_seed(STRANGER_SEED), NOW);
    let out = b
        .ingest_local(&w.sealed(&ev, STRANGER_SEED, 1, NOW), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(out.inner_id, Some(ev.id));
    assert!(!out.released);
    assert!(paired(&b));
}

#[tokio::test]
async fn a_usage_sync_applies_once() {
    let w = Ward::new();
    let b = w.broker();
    let sync = |ts: u64, g: &TestGuardian| {
        sign_event(
            &g.signer,
            kinds::CHARTER_DEVICE_USAGE_SYNC,
            ts,
            vec![kinds::marker_tag()],
            json!({
                "v": 1,
                "subject": ALICE.to_hex(),
                "ts": ts,
                "dayKey": "2023-11-14",
                "spentElsewhereTodaySecs": 1800,
            })
            .to_string(),
        )
    };
    let ev = sync(NOW - 10, &w.guardian);
    let wrap = w.wrap(&ev, 1);
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Applied
    );
    assert_eq!(
        b.ingest_local(&wrap, LOCAL).await.result,
        AckResult::Unchanged
    );
    let forged = sync(NOW - 5, &TestGuardian::from_seed(STRANGER_SEED));
    let out = b
        .ingest_local(&w.sealed(&forged, STRANGER_SEED, 2, NOW), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(out.inner_id, Some(forged.id));
}

// ---------------------------------------------------------------------------
// Serialisation with relay polling
// ---------------------------------------------------------------------------

/// Counts enact calls per reqId: unlike `time.extend`, nothing downstream
/// would hide a second enactment.
#[derive(Clone, Default)]
struct CountingEnactor(Arc<Mutex<BTreeMap<String, u32>>>);

#[async_trait]
impl Enactor for CountingEnactor {
    fn op(&self) -> OpType {
        OpType::InstallFlatpak
    }
    async fn enact(
        &self,
        grant: &VerifiedGrant,
        _ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        // Yield so the other path gets every chance to interleave.
        tokio::task::yield_now().await;
        *self
            .0
            .lock()
            .unwrap()
            .entry(grant.req_id().to_hex())
            .or_default() += 1;
        Ok(EnactOutcome { detail: None })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_ingest_racing_a_poll_enacts_each_grant_once() {
    let w = Ward::new();
    let counter = CountingEnactor::default();
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(counter.clone()));
    let b = Arc::new(w.broker_at(NOW, reg, None));

    const ROUNDS: u32 = 8;
    for round in 0..ROUNDS {
        b.submit_as(
            OpType::InstallFlatpak,
            json!({"ref": "org.test.App"}),
            None,
            Some(round),
        )
        .await
        .unwrap();
        let rec = b
            .status("")
            .into_iter()
            .find(|r| r.state == RequestState::Pending)
            .expect("the new ask");
        let grant = GrantBuilder::install_allow(rec.req_id, rec.nonce).build(&w.guardian);
        let wrap = w.wrap(&grant, round as u8 + 1);
        w.relay.inject(wrap.clone());

        let (bp, bl) = (b.clone(), b.clone());
        let poll = tokio::spawn(async move { bp.poll_once().await });
        let local = tokio::spawn(async move { bl.ingest_local(&wrap, LOCAL).await });
        poll.await.unwrap();
        let out = local.await.unwrap();
        assert!(
            matches!(out.result, AckResult::Applied | AckResult::Unchanged),
            "round {round}: {:?}",
            out.result
        );
        assert_eq!(
            b.status(&rec.req_id.to_hex())[0].state,
            RequestState::Enacted
        );
    }
    let counts = counter.0.lock().unwrap().clone();
    assert_eq!(counts.len(), ROUNDS as usize);
    assert!(counts.values().all(|n| *n == 1), "{counts:?}");
}

// ---------------------------------------------------------------------------
// Outbound: the broker's own wraps reach the local sink
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Sink(Mutex<Vec<LocalOffer>>);

impl LocalSink for Sink {
    fn offer(&self, offer: LocalOffer) {
        self.0.lock().unwrap().push(offer);
    }
}

#[tokio::test]
async fn the_brokers_request_and_audit_wraps_are_offered_and_also_published() {
    let w = Ward::new();
    let sink = Arc::new(Sink::default());
    let (reg, _inbox) = time_extend_enactors();
    let b = w.broker_at(NOW, reg, Some(sink.clone()));
    b.submit(OpType::TimeExtend, json!({}), None).await.unwrap();
    let rec = only_request(&b);
    b.ingest_local(&w.wrap(&extend_grant(&w, &rec), 1), LOCAL)
        .await;

    let offers = sink.0.lock().unwrap().clone();
    assert_eq!(offers[0].inner_kind, kinds::CHARTER_DEVICE_REQUEST);
    assert!(offers[1..]
        .iter()
        .all(|o| o.inner_kind == kinds::CHARTER_DEVICE_AUDIT));
    assert!(offers.len() >= 2, "the enactment was audited");
    // Every offered wrap went to the relays too, as the identical event.
    let published = w
        .relay
        .query(
            &[],
            Filter {
                kinds: vec![kinds::GIFT_WRAP],
                p_tags: vec![w.guardian.pubkey()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let offered: Vec<NostrEvent> = offers.into_iter().map(|o| o.wrap).collect();
    assert_eq!(published, offered);
}
