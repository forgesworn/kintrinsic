//! The full mocked broker loop: submit -> guardian delivers a grant ->
//! verify -> enact -> audit, with the security cases (enact-from-grant,
//! deny, tampered, wrong-authority, replay, offline-then-grant) and boot
//! resubscribe.

#![cfg(feature = "mock")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use charter_primitives::{Nonce, PubKey, ReqId};
use charter_proto::{GrantParams, OpType};
use charter_sys::MockSystem;
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charter_verify::VerifiedGrant;
use charterd::ports::NullEventSink;
use charterd::{
    Broker, EnactError, EnactOutcome, Enactor, EnactorRegistry, RequestRecord, RequestState,
};

const NOW: u64 = 1_700_001_000;

/// An enactor that records the install ref it received from the SIGNED grant.
struct RecordingEnactor {
    refs: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Enactor for RecordingEnactor {
    fn op(&self) -> OpType {
        OpType::InstallFlatpak
    }
    async fn enact(
        &self,
        grant: &VerifiedGrant,
        _ctx: &charterd::enactor::EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        if let Some(GrantParams::InstallFlatpak(p)) = grant.allow_params() {
            self.refs.lock().expect("lock").push(p.reference.clone());
        }
        Ok(EnactOutcome::default())
    }
}

struct Harness {
    refs: Arc<Mutex<Vec<String>>>,
    guardian: TestGuardian,
}

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
    Harness,
) {
    let sys = MockSystem::new(NOW);
    let guardian = TestGuardian::new();
    let transport = charterd::MockTransport::new(guardian.pubkey(), machine_pk());
    let refs = Arc::new(Mutex::new(Vec::new()));
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(RecordingEnactor { refs: refs.clone() }));
    let broker = Broker::new(
        sys,
        transport,
        ScriptedEntropy::new(1),
        reg,
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    (
        broker,
        Harness {
            refs,
            guardian: TestGuardian::new(),
        },
    )
}

/// Read the reqId + nonce the broker assigned to a submitted request.
fn ids_of(
    broker: &Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>,
    req_hex: &str,
) -> (ReqId, Nonce) {
    let rec = broker.status(req_hex).pop().expect("record");
    (rec.req_id, rec.nonce)
}

#[tokio::test]
async fn golden_install_loop_enacts() {
    let (broker, h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "org.req.X", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let hex = req.to_hex();
    assert_eq!(broker.status(&hex)[0].state, RequestState::Pending);

    let (rid, non) = ids_of(&broker, &hex);
    let grant = GrantBuilder::install_allow(rid, non)
        .params(serde_json::json!({"ref": "org.test.App", "remote": "flathub"}))
        .build(&h.guardian);
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;

    assert_eq!(broker.status(&hex)[0].state, RequestState::Enacted);
    assert_eq!(h.refs.lock().unwrap().len(), 1);
    // An audit was emitted.
    assert!(!broker.transport().audits().is_empty());
}

#[tokio::test]
async fn enact_from_grant_not_request() {
    let (broker, h) = build_broker();
    // Request asks for one ref; the guardian-signed grant approves a DIFFERENT
    // ref. The enactor must act on the GRANT's ref.
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "org.req.Asked", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(rid, non)
        .params(serde_json::json!({"ref": "org.grant.Approved", "remote": "flathub"}))
        .build(&h.guardian);
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(
        h.refs.lock().unwrap().as_slice(),
        &["org.grant.Approved".to_string()]
    );
}

#[tokio::test]
async fn deny_grant_no_enact() {
    let (broker, h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(rid, non)
        .deny()
        .build(&h.guardian);
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Denied);
    assert!(h.refs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn tampered_params_rejected() {
    let (broker, h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let mut grant = GrantBuilder::install_allow(rid, non)
        .params(serde_json::json!({"ref": "org.test.App", "remote": "flathub"}))
        .build(&h.guardian);
    // A hostile relay swaps the ref but cannot re-sign — the id no longer matches.
    grant.content = grant.content.replace("org.test.App", "org.evil.Swapped");
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;
    // M6: the tampered grant fails verification (the swap breaks the event id /
    // signature) and is IGNORED — the request stays Pending so the guardian's
    // genuine grant can still land. The load-bearing property: nothing enacts.
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Pending);
    assert!(h.refs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_authority_rejected() {
    let (broker, h) = build_broker();
    let attacker = TestGuardian::from_seed(0x99);
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(rid, non).build(&attacker);
    broker.transport().deliver_grant(grant, attacker.pubkey());
    broker.poll_once().await;
    // M6: a grant from a non-pinned key is IGNORED (stays Pending, never
    // enacts) — not terminally rejected, so a hostile relay can't strand the
    // request by racing a wrong-authority grant ahead of the guardian's.
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Pending);
    assert!(h.refs.lock().unwrap().is_empty());
    let _ = h.guardian;
}

#[tokio::test]
async fn forged_grant_then_genuine_still_enacts() {
    // M6 liveness regression: a hostile relay races a FORGED grant ahead of the
    // guardian's real one. The forgery is ignored (Pending, nothing enacted) and
    // the genuine grant that arrives afterwards still enacts.
    let (broker, h) = build_broker();
    let attacker = TestGuardian::from_seed(0x99);
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());

    // Forged grant first — ignored.
    let forged = GrantBuilder::install_allow(rid, non).build(&attacker);
    broker.transport().deliver_grant(forged, attacker.pubkey());
    broker.poll_once().await;
    assert_eq!(
        broker.status(&req.to_hex())[0].state,
        RequestState::Pending,
        "a forged grant must not strand the request"
    );
    assert!(h.refs.lock().unwrap().is_empty());

    // The genuine guardian grant arrives later -> enacts.
    let genuine = GrantBuilder::install_allow(rid, non)
        .params(serde_json::json!({"ref": "org.test.App", "remote": "flathub"}))
        .build(&h.guardian);
    broker
        .transport()
        .deliver_grant(genuine, h.guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Enacted);
    assert_eq!(
        h.refs.lock().unwrap().clone(),
        vec!["org.test.App".to_string()]
    );
}

#[tokio::test]
async fn boot_reconciles_stranded_enacting_to_failed() {
    // M9: a record left Enacting by a crash mid-enact cannot resume — the
    // verified grant is gone (consumed, never persisted). On boot it must
    // reconcile to a terminal Failed so it is not stuck "Enacting" forever.
    use charter_sys::{persistence::PendingStore, SystemLayer};
    let sys = MockSystem::new(NOW);
    let rec = RequestRecord {
        caller_uid: None,
        req_id: ReqId::from_bytes([7; 32]),
        nonce: Nonce::from_bytes([8; 32]),
        op: OpType::InstallFlatpak,
        state: RequestState::Enacting,
        created_at: NOW,
        detail: Some("interrupted".into()),
        source_path: None,
    };
    let key = rec.req_id.to_hex();
    sys.pending()
        .put(&key, &serde_json::to_string(&rec).unwrap())
        .unwrap();
    let disk = sys.disk();

    // "Restart": a fresh broker over the same disk reconciles on boot.
    let guardian = TestGuardian::new();
    let transport = charterd::MockTransport::new(guardian.pubkey(), machine_pk());
    let broker = Broker::new(
        MockSystem::over_disk(NOW, disk),
        transport,
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    assert_eq!(
        broker.status(&key)[0].state,
        RequestState::Failed,
        "a stranded Enacting record must reconcile to Failed on boot"
    );
}

#[tokio::test]
async fn replay_grant_no_double_enact() {
    let (broker, h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(rid, non).build(&h.guardian);
    broker
        .transport()
        .deliver_grant(grant.clone(), h.guardian.pubkey());
    broker.poll_once().await;
    // Re-deliver the same grant.
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(
        h.refs.lock().unwrap().len(),
        1,
        "single-use: no double enact"
    );
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Enacted);
}

#[tokio::test]
async fn offline_then_grant() {
    let (broker, h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    // Poll while offline — nothing delivered.
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Pending);
    // The grant lands later.
    let (rid, non) = ids_of(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(rid, non).build(&h.guardian);
    broker.transport().deliver_grant(grant, h.guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Enacted);
}

#[tokio::test]
async fn boot_resubscribe_reloads_pending() {
    let (broker, _h) = build_broker();
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let disk = broker.sys().disk();
    drop(broker);

    // Restart: a fresh broker over the same disk reloads the pending record.
    let sys2 = MockSystem::over_disk(NOW, disk);
    let transport2 = charterd::MockTransport::new(TestGuardian::new().pubkey(), machine_pk());
    let broker2 = Broker::new(
        sys2,
        transport2,
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    assert_eq!(
        broker2.status(&req.to_hex()).len(),
        1,
        "pending survives restart"
    );
}
