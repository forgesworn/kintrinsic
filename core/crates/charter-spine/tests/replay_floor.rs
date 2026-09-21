//! The monotonic `issuedAt` floor is the whole of the broker's rollback and
//! replay protection, and it is read from the clause store on every delivery.
//!
//! A store that cannot be read is NOT a store with nothing in it. These tests
//! hold the two apart: an unreadable floor refuses the event, an absent floor
//! (a slot nothing has ever been written to) still accepts it exactly as
//! before. Without the first, a hostile relay only has to arrange an EIO — or
//! wait for a half-written record — to replay last month's wider schedule.

#![cfg(feature = "mock")]

use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_proto::{ClauseKind, USAGE_SYNC_STORE_KEY};
use charter_spine::ports::NullEventSink;
use charter_spine::{Broker, EnactorRegistry, MockTransport};
use charter_sys::persistence::{ChildClauseStore, ClauseStore};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{sign_event, ClauseBuilder, TestGuardian};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const ALICE: PubKey = PubKey::from_bytes([0xA1; 32]);

fn machine_pk() -> PubKey {
    PubKey::from_bytes([0x42; 32])
}

fn broker(guardian: &TestGuardian) -> Broker<MockSystem, MockTransport, ScriptedEntropy> {
    Broker::new(
        MockSystem::new(NOW),
        MockTransport::new(guardian.pubkey(), machine_pk()),
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    )
}

fn sync_event(g: &TestGuardian, ts: u64) -> NostrEvent {
    let content = json!({
        "v": 1,
        "subject": ALICE.to_hex(),
        "ts": ts,
        "dayKey": "2023-11-14",
        "spentElsewhereTodaySecs": 1800,
    })
    .to_string();
    sign_event(
        &g.signer,
        kinds::CHARTER_DEVICE_USAGE_SYNC,
        ts,
        vec![kinds::marker_tag()],
        content,
    )
}

#[tokio::test]
async fn a_per_child_clause_is_refused_when_its_floor_cannot_be_read() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.sys().disk().break_clause_reads();

    let clause = ClauseBuilder::schedule(100).subject(ALICE).build(&guardian);
    b.transport().deliver_clause(clause, guardian.pubkey());
    let counts = b.poll_once().await;

    assert_eq!(
        counts.clauses_accepted, 0,
        "a clause whose replay floor is unreadable must be refused, not accepted with no floor"
    );
    assert_eq!(
        counts.clauses_seen, 1,
        "it was seen — it is the floor, not the payload, that could not be read"
    );
    b.sys().disk().repair_clause_reads();
    assert_eq!(
        b.sys()
            .child_clauses()
            .get_child_clause(&ALICE.to_hex(), ClauseKind::Schedule.store_key())
            .unwrap(),
        None,
        "and nothing was written: the refusal is before the store, not after it"
    );
}

#[tokio::test]
async fn a_per_child_clause_is_accepted_when_its_floor_is_merely_absent() {
    // The control: an empty slot is `Ok(None)` and still means "no previous",
    // so first delivery lands exactly as it always has.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);

    let clause = ClauseBuilder::schedule(100).subject(ALICE).build(&guardian);
    b.transport().deliver_clause(clause, guardian.pubkey());
    let counts = b.poll_once().await;

    assert_eq!(counts.clauses_accepted, 1);
    assert!(b
        .sys()
        .child_clauses()
        .get_child_clause(&ALICE.to_hex(), ClauseKind::Schedule.store_key())
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_single_child_clause_is_refused_when_its_floor_cannot_be_read() {
    // `content` is machine-wide and keeps the single-child `ClauseStore` path,
    // which reads its own floor and had the same flatten.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.sys().disk().break_clause_reads();

    let clause = ClauseBuilder::content(100).build(&guardian);
    b.transport().deliver_clause(clause, guardian.pubkey());
    let counts = b.poll_once().await;

    assert_eq!(counts.clauses_accepted, 0);
    b.sys().disk().repair_clause_reads();
    assert_eq!(
        b.sys()
            .clauses()
            .get_clause(ClauseKind::Content.store_key())
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_usage_sync_is_refused_when_its_floor_cannot_be_read() {
    // Here the floor IS the entire replay protection: a consolidated view has
    // no other freshness check, so accepting one against "no previous" lets a
    // stale view stand as current.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.sys().disk().break_clause_reads();

    b.transport()
        .deliver_usage_sync(sync_event(&guardian, 500), guardian.pubkey());
    b.poll_once().await;

    b.sys().disk().repair_clause_reads();
    assert_eq!(
        b.sys()
            .child_clauses()
            .get_child_clause(&ALICE.to_hex(), USAGE_SYNC_STORE_KEY)
            .unwrap(),
        None,
        "an unreadable floor must refuse the view, not cache it as current"
    );
}

#[tokio::test]
async fn a_usage_sync_is_stored_when_its_floor_is_merely_absent() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);

    b.transport()
        .deliver_usage_sync(sync_event(&guardian, 500), guardian.pubkey());
    b.poll_once().await;

    assert!(
        b.sys()
            .child_clauses()
            .get_child_clause(&ALICE.to_hex(), USAGE_SYNC_STORE_KEY)
            .unwrap()
            .is_some(),
        "an empty slot still means 'no previous' and accepts the first sync"
    );
    // And the single-child store is untouched by either path.
    assert_eq!(
        b.sys()
            .clauses()
            .get_clause(ClauseKind::Schedule.store_key())
            .unwrap(),
        None
    );
}
