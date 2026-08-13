//! The full mocked transport loop driving `charter-verify`: a guardian-signed
//! grant/clause is delivered gift-wrapped, unwrapped, and authenticated; a
//! hostile relay can deliver forged events but they are rejected; audit content
//! is always empty.

#![cfg(feature = "mock")]

use charter_primitives::{Nonce, NostrEvent, PubKey, ReqId};
use charter_proto::{ClauseKind, OpType};
use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
use charter_sys::relay::MockRelayTransport;
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::{ClauseBuilder, GrantBuilder, TestGuardian};
use charter_verify::{verify_clause, verify_grant, ClauseError, VerifyError, VerifyParams};

const NOW: u64 = 1_700_001_000;

fn secret(b: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = b;
    s
}

fn rid() -> ReqId {
    ReqId::from_bytes([0xA1; 32])
}
fn non() -> Nonce {
    Nonce::from_bytes([0xB2; 32])
}

/// A guardian (or attacker) at `author_sk` seals `ev` and wraps it to `recipient`.
fn deliver(
    relay: &MockRelayTransport,
    ev: &NostrEvent,
    author_sk: &[u8; 32],
    recipient: &PubKey,
    salt: u8,
) {
    let rumor = Rumor::from_signed_event(ev);
    let r = WrapRandomness {
        ephemeral_secret: secret(salt | 0x40),
        seal_nonce: [salt; 32],
        wrap_nonce: [salt.wrapping_add(1); 32],
        seal_created_at: NOW,
        wrap_created_at: NOW,
    };
    let wrap = nip59::wrap(&rumor, author_sk, recipient.as_bytes(), &r).unwrap();
    relay.inject(wrap);
}

fn transport(
    relay: MockRelayTransport,
    machine_sk: [u8; 32],
    guardian_pk: PubKey,
) -> CharterTransport<MockRelayTransport, ScriptedEntropy> {
    CharterTransport::new(
        relay,
        ScriptedEntropy::new(7),
        machine_sk,
        guardian_pk,
        vec!["wss://r".into()],
    )
}

#[tokio::test]
async fn golden_grant_delivered_and_verified() {
    let guardian = TestGuardian::new();
    let guardian_sk = secret(0x11);
    let machine_sk = secret(0x22);
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).unwrap());

    let relay = MockRelayTransport::new();
    let grant_ev = GrantBuilder::install_allow(rid(), non()).build(&guardian);
    deliver(&relay, &grant_ev, &guardian_sk, &machine_pk, 0x01);

    let t = transport(relay, machine_sk, guardian.pubkey());
    let grants = t.poll_grants(0, NOW).await.unwrap();
    assert_eq!(grants.len(), 1);

    let store = MockConsumedIdStore::new(MockDisk::new());
    let gpk = guardian.pubkey();
    let vp = VerifyParams {
        pinned_guardian: &gpk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert!(verify_grant(&grants[0].grant, &vp, &store).is_ok());
}

#[tokio::test]
async fn loop_hostile_relay_cannot_forge_grant() {
    // An attacker seals a self-signed grant and the relay serves it. It is
    // delivered, but verification against the pinned guardian rejects it.
    let guardian = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let attacker_sk = secret(0x99);
    let machine_sk = secret(0x22);
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).unwrap());

    let relay = MockRelayTransport::new();
    let forged = GrantBuilder::install_allow(rid(), non()).build(&attacker);
    deliver(&relay, &forged, &attacker_sk, &machine_pk, 0x02);

    let t = transport(relay, machine_sk, guardian.pubkey());
    let grants = t.poll_grants(0, NOW).await.unwrap();
    assert_eq!(grants.len(), 1, "the forged grant IS delivered");

    let store = MockConsumedIdStore::new(MockDisk::new());
    let gpk = guardian.pubkey();
    let vp = VerifyParams {
        pinned_guardian: &gpk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&grants[0].grant, &vp, &store),
        Err(VerifyError::UntrustedSigner),
        "but a hostile relay can never forge a grant the device accepts"
    );
}

#[tokio::test]
async fn loop_hostile_relay_cannot_set_schedule() {
    let guardian = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let attacker_sk = secret(0x99);
    let machine_sk = secret(0x22);
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).unwrap());

    let relay = MockRelayTransport::new();
    let forged = ClauseBuilder::schedule(9999).build(&attacker); // "always allow" forgery
    deliver(&relay, &forged, &attacker_sk, &machine_pk, 0x03);

    let t = transport(relay, machine_sk, guardian.pubkey());
    let clauses = t.poll_clauses(0, NOW).await.unwrap();
    assert_eq!(clauses.len(), 1, "the forged clause IS delivered");

    let gpk = guardian.pubkey();
    assert_eq!(
        verify_clause(&clauses[0].clause, &gpk, ClauseKind::Schedule, None, NOW),
        Err(ClauseError::UntrustedSigner),
        "a hostile relay can never set the schedule"
    );
}

#[tokio::test]
async fn audit_content_always_empty() {
    let guardian = TestGuardian::new();
    let guardian_sk = secret(0x11);
    let machine_sk = secret(0x22);

    let relay = MockRelayTransport::new();
    let t = transport(relay.clone(), machine_sk, guardian.pubkey());
    t.emit_audit(vec![vec!["outcome".into(), "enacted".into()]], NOW)
        .await;

    // The guardian unwraps the audit wrap; its rumor content must be empty.
    let filter = charter_sys::relay::Filter {
        kinds: vec![charter_primitives::kinds::GIFT_WRAP],
        ..Default::default()
    };
    use charter_sys::relay::RelayTransport;
    let wraps = relay.query(&["wss://r".into()], filter).await.unwrap();
    assert_eq!(wraps.len(), 1);
    let rumor = nip59::unwrap(&wraps[0], &guardian_sk, NOW, nip59::MAX_JITTER_SECS).unwrap();
    assert_eq!(rumor.kind, charter_primitives::kinds::CHARTER_DEVICE_AUDIT);
    assert_eq!(rumor.content, "", "audit content must always be empty");
}

#[tokio::test]
async fn offline_poll_returns_nothing() {
    let guardian = TestGuardian::new();
    let machine_sk = secret(0x22);
    let relay = MockRelayTransport::new();
    let t = transport(relay, machine_sk, guardian.pubkey());
    assert!(t.poll_grants(0, NOW).await.unwrap().is_empty());
}

/// Scan-to-pair: the guardian offers to be pinned, echoing the one-time token
/// from the ward's on-screen QR. The ward polls it back and — crucially —
/// learns WHO sealed it, which is the only key it may safely pin.
#[tokio::test]
async fn pair_offer_round_trips_and_reports_the_seal_author() {
    let guardian_sk = secret(0x31);
    let machine_sk = secret(0x32);
    let guardian_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&guardian_sk).unwrap());
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).unwrap());

    let relay = MockRelayTransport::new();
    // The guardian's transport seals AS the guardian and addresses the ward.
    let guardian_side = transport(relay.clone(), guardian_sk, machine_pk);
    let ward_side = transport(relay, machine_sk, guardian_pk);

    let token = "ab".repeat(16);
    guardian_side.send_pair_offer(&token, NOW).await;

    let got = ward_side.poll_pair_offers(0, NOW).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].offer.token, token);
    assert_eq!(
        got[0].seal_author, guardian_pk,
        "the ward must learn the real sealing key, not a payload claim"
    );
}

/// Junk posted to an unpaired ward's open mailbox is dropped, not fatal.
#[tokio::test]
async fn a_malformed_pair_offer_is_ignored() {
    let guardian_sk = secret(0x41);
    let machine_sk = secret(0x42);
    let guardian_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&guardian_sk).unwrap());
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).unwrap());

    let relay = MockRelayTransport::new();
    let ward_side = transport(relay.clone(), machine_sk, guardian_pk);

    // A well-formed wrap carrying a PAIR_OFFER whose token is nonsense.
    let rumor = Rumor {
        id: None,
        pubkey: guardian_pk,
        created_at: NOW,
        kind: charter_primitives::kinds::CHARTER_DEVICE_PAIR_OFFER,
        tags: vec![charter_primitives::kinds::marker_tag()],
        content: r#"{"token":"nope","relays":["wss://r"],"ts":1}"#.to_string(),
        sig: None,
    };
    let r = WrapRandomness {
        ephemeral_secret: secret(0x77),
        seal_nonce: [0x51; 32],
        wrap_nonce: [0x52; 32],
        seal_created_at: NOW,
        wrap_created_at: NOW,
    };
    relay.inject(nip59::wrap(&rumor, &guardian_sk, machine_pk.as_bytes(), &r).unwrap());

    assert!(ward_side.poll_pair_offers(0, NOW).await.unwrap().is_empty());
}
