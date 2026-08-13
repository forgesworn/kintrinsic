//! The stand-down flow as charterd composes it: a guardian-signed `standdown`
//! CLAUSE ingested by the broker lands in the per-child store, the shared
//! grace rule (`charter_spine::standdown`) turns it into the enforcer cap, the
//! multi-child tick locks with `LockReason::StandDown` once the grace elapses
//! — and a lift lets the ward straight back on, even on a device whose clock
//! runs behind the guardian's.
//!
//! Exists because the original wiring shipped as a `stand_down: None` compile
//! stub that every unit test happily passed around (fixed in 1ffa7a7): the
//! pieces were each tested, the composition was not.

#![cfg(feature = "mock")]

use charter_primitives::PubKey;
use charter_schedule::LockReason;
use charter_spine::child_policy::{EffectivePolicy, PolicySource};
use charter_spine::multi_child::MultiChildEnforcer;
use charter_spine::standdown::{stand_down_now, ChildSlot};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{ClauseBuilder, TestGuardian};
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, MockTransport};

const NOW: u64 = 1_700_001_000;
const ALICE: PubKey = PubKey::from_bytes([0xA1; 32]);
const ALICE_UID: u32 = 1001;

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

/// An unconstrained ward — no schedule, no budget. The contract's "an
/// unbounded charter is capped too": even a ward with no rules is stoppable.
fn unruled_multi() -> MultiChildEnforcer {
    let mut multi = MultiChildEnforcer::new();
    multi.sync(
        &[(
            ALICE_UID,
            EffectivePolicy {
                schedule: None,
                budget: None,
                learning: None,
                source: PolicySource::Unconstrained,
            },
        )],
        NOW as i64,
        |_| None,
    );
    multi
}

/// The level-triggered per-tick refresh the charterd runtime loop performs
/// (runtime.rs, step 1a-bis): read the verified store, feed the enforcer.
fn refresh(
    multi: &mut MultiChildEnforcer,
    b: &Broker<MockSystem, MockTransport, ScriptedEntropy>,
    now: u64,
) {
    multi.set_stand_down(
        ALICE_UID,
        stand_down_now(
            &ChildSlot {
                store: b.sys().child_clauses(),
                subject_hex: &ALICE.to_hex(),
            },
            now,
        ),
    );
}

fn locked_reason(multi: &mut MultiChildEnforcer, now: u64) -> (bool, Option<LockReason>) {
    let d = multi.tick(Some(ALICE_UID), now as i64, 2);
    let alice = d.iter().find(|c| c.uid == ALICE_UID).expect("alice ticked");
    (alice.locked, alice.reason)
}

#[tokio::test]
async fn a_stand_down_travels_store_to_lock_and_a_lift_frees_her() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let mut multi = unruled_multi();

    // Before anything arrives: unlocked, and the refresh finds nothing.
    refresh(&mut multi, &b, NOW);
    let (locked, _) = locked_reason(&mut multi, NOW);
    assert!(!locked, "no stand-down, no lock");

    // The guardian calls "finish up now" — signed clause, broker-ingested.
    let call = ClauseBuilder::stand_down(NOW, "sd-flow", NOW + 4 * 3600)
        .subject(ALICE)
        .build(&guardian);
    b.transport().deliver_clause(call, guardian.pubkey());
    b.poll_once().await;

    // First sight: the ward is owed her minute, not the lock.
    refresh(&mut multi, &b, NOW);
    let (locked, _) = locked_reason(&mut multi, NOW);
    assert!(!locked, "the grace comes before the lock");

    // Past the grace: locked, and named for the person who called it.
    refresh(&mut multi, &b, NOW + 61);
    let (locked, reason) = locked_reason(&mut multi, NOW + 61);
    assert!(locked, "a ward with no rules must still be stoppable");
    assert_eq!(reason, Some(LockReason::StandDown));

    // "Allow back on": the SAME clause kind, expiry at its own issue instant,
    // stamped by a guardian clock 90 seconds AHEAD of this device. The lift
    // must free her — never read as a fresh stand-down that re-locks her.
    let device_now = NOW + 120;
    let guardian_now = device_now + 90;
    let lift = ClauseBuilder::stand_down(guardian_now, "sd-lift", guardian_now)
        .subject(ALICE)
        .build(&guardian);
    b.transport().deliver_clause(lift, guardian.pubkey());
    b.poll_once().await;

    refresh(&mut multi, &b, device_now);
    let (locked, _) = locked_reason(&mut multi, device_now);
    assert!(!locked, "the lift must free the ward, even on a slow clock");
}
