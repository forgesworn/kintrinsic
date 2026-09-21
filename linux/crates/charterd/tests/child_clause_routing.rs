//! Per-child CLAUSE routing in the broker (multi-child, Signet-first).
//!
//! A guardian CLAUSE carrying `subject` is authenticated against the pinned
//! guardian exactly as before, but cached under the per-(subject, kind)
//! `ChildClauseStore` instead of the single-child `ClauseStore`. A subject-less
//! clause keeps the legacy single-child path. Rollback protection and the
//! pinned-authority + fail-closed invariants hold per child.

#![cfg(feature = "mock")]

use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_sys::persistence::{ChildClauseStore, ClauseStore};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{ClauseBuilder, TestGuardian};
use charterd::child_policy::{resolve_child_policies, PolicySource};
use charterd::device_limits::{ChildConfig, DeviceLimits};
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, MockTransport};

const PASSWD: &str = "root:x:0:0::/root:/bin/bash\nalice:x:1001:1001::/home/alice:/bin/bash\n";

fn dl() -> DeviceLimits {
    DeviceLimits {
        tz: "UTC".into(),
        wake: "07:00".into(),
        bedtime: "20:00".into(),
        daily_minutes: 60,
        weekend: None,
    }
}

const NOW: u64 = 1_700_001_000;

fn machine_pk() -> PubKey {
    PubKey::from_bytes([0x42; 32])
}
const ALICE: PubKey = PubKey::from_bytes([0xA1; 32]);
const BOB: PubKey = PubKey::from_bytes([0xB0; 32]);
/// The pairing's sole subject — the `subject` the broker is constructed with
/// (see `broker()` below), i.e. the back-compat target for subject-less clauses.
const SOLE: PubKey = PubKey::from_bytes([0xBB; 32]);

fn broker(guardian: &TestGuardian) -> Broker<MockSystem, MockTransport, ScriptedEntropy> {
    let sys = MockSystem::new(NOW);
    let transport = MockTransport::new(guardian.pubkey(), machine_pk());
    Broker::new(
        sys,
        transport,
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    )
}

#[tokio::test]
async fn clause_with_subject_routes_to_child_store_not_single() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let clause = ClauseBuilder::schedule(100).subject(ALICE).build(&guardian);
    b.transport().deliver_clause(clause, guardian.pubkey());
    b.poll_once().await;

    // Landed in Alice's per-child slot...
    let stored = b
        .sys()
        .child_clauses()
        .get_child_clause(&ALICE.to_hex(), ClauseKind::Schedule.store_key())
        .unwrap();
    assert!(stored.is_some(), "alice's schedule cached per-child");
    // ...and NOT in the single-child store.
    assert_eq!(
        b.sys()
            .clauses()
            .get_clause(ClauseKind::Schedule.store_key())
            .unwrap(),
        None,
        "per-child clause must not leak into the single-child store"
    );
}

#[tokio::test]
async fn subjectless_screentime_clause_routes_to_pairing_sole_subject() {
    // A subject-less schedule/budget (the single-child back-compat wire) MUST land
    // in the per-(subject, kind) store under the pairing's sole subject, because
    // the live MultiChildEnforcer reads ONLY that store. Stranding it in the
    // single-child ClauseStore (which nothing in the enforce loop reads) is a
    // silent fail-open: authenticated + cached, but never freezes/locks anyone.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let clause = ClauseBuilder::schedule(100).build(&guardian); // no subject
    b.transport().deliver_clause(clause, guardian.pubkey());
    b.poll_once().await;

    assert!(
        b.sys()
            .child_clauses()
            .get_child_clause(&SOLE.to_hex(), ClauseKind::Schedule.store_key())
            .unwrap()
            .is_some(),
        "subject-less schedule must enforce via the pairing's sole subject"
    );
    assert_eq!(
        b.sys()
            .clauses()
            .get_clause(ClauseKind::Schedule.store_key())
            .unwrap(),
        None,
        "a screen-time clause must NOT be stranded in the unenforced single-child store"
    );
    // And it didn't leak into some other child's slot.
    assert_eq!(
        b.sys()
            .child_clauses()
            .clauses_for(&ALICE.to_hex())
            .unwrap()
            .clauses
            .len(),
        0
    );
}

#[tokio::test]
async fn poll_cursor_lags_by_jitter_window_so_late_events_are_not_skipped() {
    // The next poll's `since` cursor must sit a full NIP-59 jitter window behind
    // wall-clock — never at bare `now` — or the relay's `since` filter would
    // permanently drop a slightly-backdated or late-redelivered guardian event.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.poll_once().await;
    assert_eq!(
        b.cursor(),
        NOW - charter_transport::nip59::MAX_JITTER_SECS,
        "cursor must lag wall-clock by the transport jitter window, not sit at now"
    );
}

#[tokio::test]
async fn subjectless_content_clause_stays_machine_wide() {
    // Content (web filtering) is machine-wide and the web-content enforcer reads
    // the single-child ClauseStore directly — so a subject-less content clause
    // must stay there, NOT get routed per-child.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let clause = ClauseBuilder::content(100).build(&guardian); // no subject
    b.transport().deliver_clause(clause, guardian.pubkey());
    b.poll_once().await;

    assert!(
        b.sys()
            .clauses()
            .get_clause(ClauseKind::Content.store_key())
            .unwrap()
            .is_some(),
        "machine-wide content stays on the single-child store"
    );
    assert_eq!(
        b.sys()
            .child_clauses()
            .clauses_for(&SOLE.to_hex())
            .unwrap()
            .clauses
            .len(),
        0,
        "content is not per-child routed"
    );
}

#[tokio::test]
async fn per_child_clauses_are_independent_across_children() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_clause(
        ClauseBuilder::schedule(100).subject(ALICE).build(&guardian),
        guardian.pubkey(),
    );
    b.transport().deliver_clause(
        ClauseBuilder::budget(100).subject(BOB).build(&guardian),
        guardian.pubkey(),
    );
    b.poll_once().await;

    assert_eq!(
        b.sys()
            .child_clauses()
            .clauses_for(&ALICE.to_hex())
            .unwrap()
            .clauses
            .len(),
        1
    );
    assert_eq!(
        b.sys()
            .child_clauses()
            .clauses_for(&BOB.to_hex())
            .unwrap()
            .clauses
            .len(),
        1
    );
    // Alice has a schedule, not a budget; Bob the reverse.
    assert!(b
        .sys()
        .child_clauses()
        .get_child_clause(&ALICE.to_hex(), ClauseKind::Budget.store_key())
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn per_child_rollback_rejects_lower_issued_at() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_clause(
        ClauseBuilder::schedule(200).subject(ALICE).build(&guardian),
        guardian.pubkey(),
    );
    b.poll_once().await;
    // A replayed older clause for the same child is rejected (rollback).
    b.transport().deliver_clause(
        ClauseBuilder::schedule(100).subject(ALICE).build(&guardian),
        guardian.pubkey(),
    );
    b.poll_once().await;
    assert_eq!(
        b.sys()
            .child_clauses()
            .highest_issued_at(&ALICE.to_hex(), ClauseKind::Schedule.store_key())
            .unwrap(),
        Some(200),
        "older per-child clause must not roll the slot back"
    );
}

#[tokio::test]
async fn cached_clause_is_inert_until_bound_then_applies_when_bound() {
    // End-to-end: the broker caches a guardian clause under ALICE.to_hex(); it is
    // INERT until a ChildConfig binds a local account to ALICE's subject, then it
    // applies — proving the broker's to_hex() store key matches the config lookup
    // key (a silent casing/format drift here would degrade every adopted child to
    // device-only with no other failing test).
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_clause(
        ClauseBuilder::schedule(100).subject(ALICE).build(&guardian),
        guardian.pubkey(),
    );
    b.poll_once().await;

    // Inert: with no config binding ALICE, the cached clause governs nobody.
    assert!(
        resolve_child_policies(b.sys(), PASSWD, &[]).is_empty(),
        "a cached clause for an unbound subject enforces nothing"
    );

    // Bind alice -> ALICE's subject. The cached clause now applies (Guardian).
    let configs = vec![(
        "alice".to_string(),
        ChildConfig {
            subject: Some(ALICE.to_hex()),
            limits: Some(dl()),
            learning: None,
        },
    )];
    let out = resolve_child_policies(b.sys(), PASSWD, &configs);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, 1001);
    assert_eq!(
        out[0].1.source,
        PolicySource::Guardian,
        "the previously-cached clause applies once the binding exists"
    );
}

#[tokio::test]
async fn binding_first_then_clause_also_resolves_to_guardian() {
    // Arrival-order independence: the binding exists BEFORE the clause arrives.
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let configs = vec![(
        "alice".to_string(),
        ChildConfig {
            subject: Some(ALICE.to_hex()),
            limits: Some(dl()),
            learning: None,
        },
    )];
    // Before the clause: device-only fallback.
    assert_eq!(
        resolve_child_policies(b.sys(), PASSWD, &configs)[0]
            .1
            .source,
        PolicySource::DeviceOnly
    );
    // Clause arrives -> Guardian.
    b.transport().deliver_clause(
        ClauseBuilder::budget(100).subject(ALICE).build(&guardian),
        guardian.pubkey(),
    );
    b.poll_once().await;
    assert_eq!(
        resolve_child_policies(b.sys(), PASSWD, &configs)[0]
            .1
            .source,
        PolicySource::Guardian
    );
}

#[tokio::test]
async fn forged_per_child_clause_is_not_cached() {
    let guardian = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let b = broker(&guardian);
    // A clause for Alice signed by the WRONG key — fail-closed, never cached.
    let forged = ClauseBuilder::schedule(100)
        .subject(ALICE)
        .build_signed_by(&attacker.signer);
    b.transport().deliver_clause(forged, attacker.pubkey());
    b.poll_once().await;
    assert_eq!(
        b.sys()
            .child_clauses()
            .clauses_for(&ALICE.to_hex())
            .unwrap()
            .clauses
            .len(),
        0,
        "a forged per-child clause must not be cached"
    );
}
