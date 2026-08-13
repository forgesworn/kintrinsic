//! The full mocked broker loop for `app.open` (C1, review round 1 2026-08-03):
//! submit -> guardian delivers the {pkg, minutesGranted} answer-signal grant
//! -> verify -> enact (a no-op — see `AppOpenEnactor`) -> `Enacted`/`Denied`.
//!
//! Before this, `app.open` had NO grant params at all: `Decision::Allow`
//! unconditionally calls `grant_params()` in `verify_grant` (step 1, before
//! the signature/id checks even run), so an allow could never verify —
//! `(Pending, GrantRejected)` is a no-op transition by design (a forged grant
//! must not strand a request), which meant a REAL allow was silently
//! indistinguishable from an attack and the request sat `Pending` forever.
//! This file proves the fixed loop reaches the SAME terminal states
//! `golden_install_loop_enacts`/`deny_grant_no_enact` (`broker_loop.rs`) prove
//! for `install.flatpak`.

#![cfg(feature = "mock")]

use charter_primitives::{Nonce, PubKey, ReqId};
use charter_proto::{AppOpenGrantParams, OpType};
use charter_sys::MockSystem;
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charterd::enactors::AppOpenEnactor;
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, RequestState};

const NOW: u64 = 1_700_001_000;

fn machine_pk() -> PubKey {
    PubKey::from_bytes(
        charter_crypto::xonly_pubkey(&{
            let mut s = [0u8; 32];
            s[31] = 0x42;
            s
        })
        .unwrap(),
    )
}

fn build_broker() -> (
    Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>,
    TestGuardian,
) {
    let sys = MockSystem::new(NOW);
    let guardian = TestGuardian::new();
    let transport = charterd::MockTransport::new(guardian.pubkey(), machine_pk());
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(AppOpenEnactor));
    let broker = Broker::new(
        sys,
        transport,
        ScriptedEntropy::new(1),
        reg,
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    (broker, guardian)
}

fn ids_of(
    broker: &Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>,
    req_hex: &str,
) -> (ReqId, Nonce) {
    let rec = broker.status(req_hex).pop().expect("record");
    (rec.req_id, rec.nonce)
}

#[tokio::test]
async fn app_open_allow_reaches_enacted() {
    let (broker, guardian) = build_broker();
    let req = broker
        .submit(
            OpType::AppOpen,
            serde_json::json!({"pkg": "com.mojang.minecraftpe"}),
            None,
        )
        .await
        .unwrap();
    let hex = req.to_hex();
    assert_eq!(broker.status(&hex)[0].state, RequestState::Pending);

    let (rid, non) = ids_of(&broker, &hex);
    let params = AppOpenGrantParams {
        pkg: "com.mojang.minecraftpe".into(),
        minutes_granted: 30,
    };
    let grant = GrantBuilder::app_open_allow(rid, non, serde_json::to_value(&params).unwrap())
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;

    // Before the C1 fix this stayed Pending forever (GrantRejected is a
    // no-op transition) — the whole point of this test.
    assert_eq!(broker.status(&hex)[0].state, RequestState::Enacted);
    assert!(!broker.transport().audits().is_empty());
}

#[tokio::test]
async fn app_open_deny_reaches_the_same_terminal_state_time_extend_uses() {
    let (broker, guardian) = build_broker();
    let req = broker
        .submit(
            OpType::AppOpen,
            serde_json::json!({"pkg": "com.mojang.minecraftpe"}),
            None,
        )
        .await
        .unwrap();
    let hex = req.to_hex();

    let (rid, non) = ids_of(&broker, &hex);
    // A deny carries minutesGranted: 0 (never parsed — Decision::Deny skips
    // grant_params entirely) but the SAME shape round-trips regardless.
    let params = AppOpenGrantParams {
        pkg: "com.mojang.minecraftpe".into(),
        minutes_granted: 0,
    };
    let grant = GrantBuilder::app_open_allow(rid, non, serde_json::to_value(&params).unwrap())
        .deny()
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;

    assert_eq!(broker.status(&hex)[0].state, RequestState::Denied);
}
