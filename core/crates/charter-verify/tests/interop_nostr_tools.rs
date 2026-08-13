//! Full interop: REAL nostr-tools-signed grants/clauses verify end-to-end in
//! Rust. The golden signatures were produced by the same library the guardian
//! app uses — never hand-written hex.

#![cfg(feature = "mock")]

use charter_primitives::{Nonce, NostrEvent, PubKey, ReqId};
use charter_proto::{ClauseKind, GrantParams, OpType};
use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
use charter_verify::{verify_clause, verify_grant, GrantOutcome, VerifyParams};

#[derive(serde::Deserialize)]
struct GrantVector {
    event: NostrEvent,
    guardian_pubkey: PubKey,
}

#[derive(serde::Deserialize)]
struct ClauseVector {
    event: NostrEvent,
    guardian_pubkey: PubKey,
}

fn known_req_id() -> ReqId {
    ReqId::from_hex(&"11".repeat(32)).unwrap()
}
fn known_nonce() -> Nonce {
    Nonce::from_hex(&"22".repeat(32)).unwrap()
}

#[test]
fn schnorr_verify_accepts_nostr_tools_triple_and_allow_params() {
    let v: GrantVector = charter_testkit::golden::load_json("nostr/grant_install_allow.json");
    let store = MockConsumedIdStore::new(MockDisk::new());
    let p = VerifyParams {
        pinned_guardian: &v.guardian_pubkey,
        expected_req_id: &known_req_id(),
        expected_nonce: &known_nonce(),
        expected_op: OpType::InstallFlatpak,
        now: 1_700_001_000,
    };
    let vg = verify_grant(&v.event, &p, &store).unwrap();
    match vg.outcome() {
        GrantOutcome::Allow(GrantParams::InstallFlatpak(ip)) => {
            assert_eq!(ip.reference, "org.videolan.VLC");
        }
        _ => panic!("expected install allow"),
    }
}

#[test]
fn nostr_tools_deny_grant_verifies_as_deny() {
    let v: GrantVector = charter_testkit::golden::load_json("nostr/grant_install_deny.json");
    let store = MockConsumedIdStore::new(MockDisk::new());
    let p = VerifyParams {
        pinned_guardian: &v.guardian_pubkey,
        expected_req_id: &known_req_id(),
        expected_nonce: &known_nonce(),
        expected_op: OpType::InstallFlatpak,
        now: 1_700_001_000,
    };
    let vg = verify_grant(&v.event, &p, &store).unwrap();
    assert_eq!(vg.outcome(), &GrantOutcome::Deny);
}

#[test]
fn nostr_tools_clause_authenticates() {
    let v: ClauseVector = charter_testkit::golden::load_json("nostr/clause_schedule.json");
    let vc = verify_clause(
        &v.event,
        &v.guardian_pubkey,
        ClauseKind::Schedule,
        None,
        1_700_001_000,
    )
    .unwrap();
    assert_eq!(vc.issued_at(), 1_700_000_000);
}
