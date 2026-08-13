//! Per-caller scoping of the brokered-request surface (S8, review 2026-08-07).
//!
//! A family Linux box has several children with their own accounts on it. The
//! D-Bus surface resolved the kernel-attested caller uid for `SubmitRequest`
//! and `TimeLeft` and for nothing else, so `ListRequests` handed any local
//! account every account's pending asks — what each child had asked for and
//! when — and `CancelRequest` withdrew any of them by id, with those ids
//! enumerable from the very list that leaked them.
//!
//! Nothing here was a route to unauthorised TIME: a cancel only withdraws a
//! request that has not been answered, and a grant is guardian-signed
//! regardless. It is a privacy boundary between siblings, and a small cruelty
//! ("your ask vanished before Mum saw it") that the machine should not permit.

#![cfg(feature = "mock")]

use charter_primitives::PubKey;
use charter_proto::OpType;
use charter_spine::lifecycle::RequestState;
use charter_sys::MockSystem;
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::TestGuardian;
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, MockTransport};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const ROOT: u32 = 0;
const MIA: u32 = 1000;
const ROOK: u32 = 1001;

fn broker() -> Broker<MockSystem, MockTransport, ScriptedEntropy> {
    let guardian = TestGuardian::new();
    Broker::new(
        MockSystem::new(NOW),
        MockTransport::new(guardian.pubkey(), PubKey::from_bytes([0x42; 32])),
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    )
}

async fn ask(
    b: &Broker<MockSystem, MockTransport, ScriptedEntropy>,
    uid: Option<u32>,
    minutes: u64,
) -> String {
    b.submit_as(OpType::TimeExtend, json!({ "minutes": minutes }), None, uid)
        .await
        .expect("submitted")
        .to_hex()
}

#[tokio::test]
async fn a_child_sees_their_own_asks_and_not_their_siblings() {
    let b = broker();
    let mine = ask(&b, Some(MIA), 10).await;
    let theirs = ask(&b, Some(ROOK), 20).await;

    let seen: Vec<_> = b
        .list_for(MIA, 0)
        .into_iter()
        .map(|r| r.req_id.to_hex())
        .collect();
    assert_eq!(seen, vec![mine], "one sibling's list is not the other's");
    assert!(!seen.contains(&theirs));
}

#[tokio::test]
async fn root_sees_every_ask() {
    let b = broker();
    ask(&b, Some(MIA), 10).await;
    ask(&b, Some(ROOK), 20).await;
    // charterd runs as root, and an administrator at the machine is the
    // authority every other path already defers to.
    assert_eq!(b.list_for(ROOT, 0).len(), 2);
}

#[tokio::test]
async fn querying_a_sibling_s_req_id_directly_returns_nothing() {
    let b = broker();
    let theirs = ask(&b, Some(ROOK), 20).await;
    // Knowing the id must not be enough — that was the whole exposure, since
    // the id came from a list that should not have been readable either.
    assert!(b.status_for(MIA, &theirs).is_empty());
    assert_eq!(b.status_for(ROOK, &theirs).len(), 1);
}

#[tokio::test]
async fn a_child_cannot_withdraw_a_siblings_ask() {
    let b = broker();
    let theirs = ask(&b, Some(ROOK), 20).await;

    assert!(!b.cancel_as(MIA, &theirs), "refused");
    assert_eq!(
        b.status_for(ROOK, &theirs)[0].state,
        RequestState::Pending,
        "and it is still standing, waiting for a real answer"
    );
}

#[tokio::test]
async fn a_child_can_withdraw_their_own() {
    let b = broker();
    let mine = ask(&b, Some(MIA), 10).await;
    assert!(b.cancel_as(MIA, &mine));
    assert_eq!(b.status_for(MIA, &mine)[0].state, RequestState::Cancelled);
}

#[tokio::test]
async fn root_can_withdraw_anything() {
    let b = broker();
    let theirs = ask(&b, Some(ROOK), 20).await;
    assert!(b.cancel_as(ROOT, &theirs));
}

/*
 * A refusal must be indistinguishable from "no such request": a distinct
 * answer would confirm to a probing sibling that a reqId is real.
 */
#[tokio::test]
async fn refusing_someone_elses_looks_exactly_like_a_reqid_that_does_not_exist() {
    let b = broker();
    let theirs = ask(&b, Some(ROOK), 20).await;
    assert_eq!(
        b.cancel_as(MIA, &theirs),
        b.cancel_as(MIA, &"ff".repeat(32)),
    );
}

/*
 * An UNOWNED record — a single-user platform, or one persisted before the
 * field existed — belongs to nobody. Root only. It must never be handed to a
 * local account on the strength of a missing field.
 */
#[tokio::test]
async fn an_unowned_record_is_root_only() {
    let b = broker();
    let orphan = ask(&b, None, 10).await;
    assert!(b.status_for(MIA, &orphan).is_empty());
    assert!(!b.cancel_as(MIA, &orphan));
    assert_eq!(b.status_for(ROOT, &orphan).len(), 1);
}

#[tokio::test]
async fn the_limit_counts_your_own_asks_not_the_ones_you_filtered_out() {
    let b = broker();
    for _ in 0..3 {
        ask(&b, Some(ROOK), 20).await;
    }
    for _ in 0..2 {
        ask(&b, Some(MIA), 10).await;
    }
    // Scoped first, truncated second — otherwise "show me my two most recent"
    // returns nothing at all whenever a sibling has been busier.
    assert_eq!(b.list_for(MIA, 2).len(), 2);
}
