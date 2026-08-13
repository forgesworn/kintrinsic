//! USAGE_SYNC (31115) broker ingest: guardian-pinned authentication, per-
//! subject ts-monotonic replay protection, storage under the reserved store
//! key, and the `usage_pool` load the enforce loop feeds from. A hostile
//! relay can delay/drop a sync but never forge one or roll one back.

#![cfg(feature = "mock")]

use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_proto::USAGE_SYNC_STORE_KEY;
use charter_sys::persistence::ChildClauseStore;
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{sign_event, TestGuardian};
use charterd::ports::NullEventSink;
use charterd::usage_pool::load_consolidated;
use charterd::{Broker, EnactorRegistry, MockTransport};
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

fn sync_event(g: &TestGuardian, ts: u64, spent: u64) -> NostrEvent {
    let content = json!({
        "v": 1,
        "subject": ALICE.to_hex(),
        "ts": ts,
        "dayKey": "2023-11-14",
        "spentElsewhereTodaySecs": spent,
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
async fn usage_sync_routes_verifies_and_stores() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport()
        .deliver_usage_sync(sync_event(&guardian, 500, 1800), guardian.pubkey());
    b.poll_once().await;

    let stored = b
        .sys()
        .child_clauses()
        .get_child_clause(&ALICE.to_hex(), USAGE_SYNC_STORE_KEY)
        .unwrap();
    assert!(stored.is_some(), "verified sync cached under the store key");

    let view = load_consolidated(b.sys(), &ALICE.to_hex()).expect("loads back");
    assert_eq!(view.spent_elsewhere_today_secs, 1800);
    assert_eq!(view.day_key, "2023-11-14");
}

#[tokio::test]
async fn usage_sync_stale_ts_rejected() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport()
        .deliver_usage_sync(sync_event(&guardian, 500, 1800), guardian.pubkey());
    b.poll_once().await;
    // An older (replayed) view claiming LESS spent must not roll back.
    b.transport()
        .deliver_usage_sync(sync_event(&guardian, 400, 0), guardian.pubkey());
    b.poll_once().await;

    let view = load_consolidated(b.sys(), &ALICE.to_hex()).expect("still stored");
    assert_eq!(
        view.spent_elsewhere_today_secs, 1800,
        "ts=400 replay rejected"
    );
}

#[tokio::test]
async fn usage_sync_forged_signer_not_stored() {
    let guardian = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let b = broker(&guardian);
    b.transport()
        .deliver_usage_sync(sync_event(&attacker, 500, 59_999), attacker.pubkey());
    b.poll_once().await;

    assert!(
        load_consolidated(b.sys(), &ALICE.to_hex()).is_none(),
        "an attacker-signed sync must never be cached"
    );
}
