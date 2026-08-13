//! RELEASE (31116) on the Linux warden — the guardian's remote unpair (S7,
//! review 2026-08-07).
//!
//! `poll_releases` and `verify_release` had existed since the Android warden
//! was written, and nothing in the spine or charterd ever called them. A
//! guardian pressing "Disconnect" in Kintrinsic published a perfectly valid,
//! perfectly signed release into a void: the laptop stayed managed forever
//! while the app showed the device as gone. The direction was fail-closed — a
//! ward gained nothing by it — but a management function that silently does
//! nothing is a promise the product does not keep.
//!
//! These tests are the other half of that: they prove the release now LANDS,
//! and that all the ways it must be refused still refuse it.

#![cfg(feature = "mock")]

use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_sys::persistence::{ChildClauseStore, PairingStore};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{sign_event, TestGuardian};
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, MockTransport};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const PAIRED_AT: u64 = 1_700_000_000;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);

fn machine_pk() -> PubKey {
    PubKey::from_bytes([0x42; 32])
}

fn broker(guardian: &TestGuardian) -> Broker<MockSystem, MockTransport, ScriptedEntropy> {
    let b = Broker::new(
        MockSystem::new(NOW),
        MockTransport::new(guardian.pubkey(), machine_pk()),
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        SUBJECT,
    );
    // A pinned pairing, and a clause under it — the state a release destroys.
    b.sys()
        .pairing()
        .save(
            &json!({
                "guardian_pubkey": guardian.pubkey().to_hex(),
                "relays": ["wss://relay.example"],
                "machine": machine_pk().to_hex(),
                "subject_pubkey": SUBJECT.to_hex(),
                "audit_transparency": false,
                "paired_at": PAIRED_AT,
            })
            .to_string(),
        )
        .expect("pairing saved");
    b.sys()
        .child_clauses()
        .put_child_clause(&SUBJECT.to_hex(), 1, PAIRED_AT + 1, "{\"v\":1}")
        .expect("clause stored");
    b
}

fn release_event(g: &TestGuardian, machine: PubKey, issued_at: u64) -> NostrEvent {
    let content = json!({ "v": 1, "machine": machine.to_hex(), "issuedAt": issued_at }).to_string();
    sign_event(
        &g.signer,
        kinds::CHARTER_DEVICE_RELEASE,
        issued_at,
        vec![kinds::marker_tag()],
        content,
    )
}

fn still_managed(b: &Broker<MockSystem, MockTransport, ScriptedEntropy>) -> bool {
    b.sys().pairing().load().ok().flatten().is_some()
}

#[tokio::test]
async fn a_guardian_release_unpairs_the_device_and_forgets_its_clauses() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_release(
        release_event(&guardian, machine_pk(), NOW),
        guardian.pubkey(),
    );

    let counts = b.poll_once().await;

    assert!(counts.released, "the caller must be told to stand down");
    assert!(!still_managed(&b), "the pairing is gone");
    assert!(
        b.sys()
            .child_clauses()
            .clauses_for(&SUBJECT.to_hex())
            .unwrap()
            .is_empty(),
        "and so are the rules it carried — a released device enforces nothing"
    );
}

/*
 * THE replay that was found live on 2026-07-22 (issue #49 sibling): a device
 * re-paired to the SAME guardian kept getting un-paired by its own earlier
 * release, redelivered inside the relay lookback window. The pairing epoch is
 * the floor.
 */
#[tokio::test]
async fn a_release_predating_the_current_pairing_is_refused() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_release(
        release_event(&guardian, machine_pk(), PAIRED_AT - 1),
        guardian.pubkey(),
    );

    let counts = b.poll_once().await;

    assert!(!counts.released);
    assert!(still_managed(&b), "a stale decision must not unpair us");
}

#[tokio::test]
async fn a_release_issued_after_the_pairing_epoch_still_lands() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    b.transport().deliver_release(
        release_event(&guardian, machine_pk(), PAIRED_AT + 5),
        guardian.pubkey(),
    );
    assert!(b.poll_once().await.released);
}

#[tokio::test]
async fn a_release_signed_by_anyone_else_is_refused() {
    let guardian = TestGuardian::new();
    // A DIFFERENT seed — `TestGuardian::new()` is the pinned one, every time.
    let stranger = TestGuardian::from_seed(0x5a);
    let b = broker(&guardian);
    // Sealed and delivered as if by the guardian — the SEAL author is not the
    // thing that authorises this; the inner signature is.
    b.transport().deliver_release(
        release_event(&stranger, machine_pk(), NOW),
        guardian.pubkey(),
    );

    assert!(!b.poll_once().await.released);
    assert!(still_managed(&b));
}

#[tokio::test]
async fn a_release_addressed_to_a_different_machine_is_refused() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let someone_elses = PubKey::from_bytes([0x99; 32]);
    b.transport().deliver_release(
        release_event(&guardian, someone_elses, NOW),
        guardian.pubkey(),
    );

    assert!(!b.poll_once().await.released);
    assert!(
        still_managed(&b),
        "a sibling's release is not ours to honour"
    );
}

#[tokio::test]
async fn a_long_delayed_release_is_refused_as_stale() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let ancient = NOW - charter_verify::RELEASE_FRESHNESS_SECS - 1;
    b.transport().deliver_release(
        release_event(&guardian, machine_pk(), ancient),
        guardian.pubkey(),
    );

    assert!(!b.poll_once().await.released);
    assert!(still_managed(&b));
}

/*
 * A release and a clause arriving in the same round. The release wins and the
 * round stops: writing the guardian's rules back into a store we just emptied
 * would leave a device that is unpaired and still carrying policy, with
 * nobody left who could lift it.
 */
#[tokio::test]
async fn a_clause_in_the_same_round_as_a_release_is_not_ingested() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    let clause = sign_event(
        &guardian.signer,
        kinds::CHARTER_DEVICE_CLAUSE,
        NOW,
        vec![kinds::marker_tag()],
        json!({
            "v": 1,
            "kind": "budget",
            "issuedAt": NOW,
            "subject": SUBJECT.to_hex(),
            "body": { "v": 1, "dailyMinutes": 60, "tz": "UTC" },
        })
        .to_string(),
    );
    b.transport().deliver_clause(clause, guardian.pubkey());
    b.transport().deliver_release(
        release_event(&guardian, machine_pk(), NOW),
        guardian.pubkey(),
    );

    let counts = b.poll_once().await;

    assert!(counts.released);
    assert_eq!(
        counts.clauses_accepted, 0,
        "no rules land on a freed device"
    );
    assert!(b
        .sys()
        .child_clauses()
        .clauses_for(&SUBJECT.to_hex())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn an_ordinary_round_reports_no_release() {
    let guardian = TestGuardian::new();
    let b = broker(&guardian);
    assert!(!b.poll_once().await.released);
    assert!(still_managed(&b));
}
