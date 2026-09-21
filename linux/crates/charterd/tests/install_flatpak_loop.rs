//! `install.flatpak` end-to-end: the enactor acts on the SIGNED grant ref,
//! injection is rejected before any FlatpakOps call, installs are idempotent,
//! and the full mocked loop installs exactly the granted ref once.

#![cfg(feature = "mock")]

use std::sync::Arc;

use charter_primitives::{Nonce, PubKey, ReqId};
use charter_proto::OpType;
use charter_sys::effects::MockFlatpakOps;
use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
use charter_sys::MockSystem;
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charter_verify::{verify_grant, VerifiedGrant, VerifyParams};
use charterd::enactor::Enactor;
use charterd::enactors::install_flatpak::{
    build_install_request, InstallFlatpakEnactor, RequestBuildError,
};
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, RequestState};

const NOW: u64 = 1_700_001_000;

fn rid() -> ReqId {
    ReqId::from_bytes([0xA1; 32])
}
fn non() -> Nonce {
    Nonce::from_bytes([0xB2; 32])
}

/// Verify an install.flatpak grant carrying arbitrary params.
fn try_install_grant(
    params: serde_json::Value,
) -> Result<VerifiedGrant, charter_verify::VerifyError> {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .params(params)
        .build(&g);
    let pk = g.pubkey();
    let store = MockConsumedIdStore::new(MockDisk::new());
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    verify_grant(&ev, &p, &store)
}

/// Build a VerifiedGrant for install.flatpak with arbitrary params.
fn install_grant(params: serde_json::Value) -> VerifiedGrant {
    try_install_grant(params).unwrap()
}

#[tokio::test]
async fn enactor_installs_from_grant_ref() {
    let flat = MockFlatpakOps::new();
    let enactor = InstallFlatpakEnactor::new(flat);
    let grant = install_grant(serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}));
    enactor
        .enact(&grant, &charterd::enactor::EnactContext::default())
        .await
        .unwrap();
    assert_eq!(
        enactor.flatpak().install_calls(),
        vec!["org.videolan.VLC".to_string()]
    );
}

/// An injection-shaped ref no longer even VERIFIES: `GrantParams::parse` runs
/// it through `FlatpakRef` at the trust boundary, so no enactor — this one or
/// a future one — is ever handed it. (The enactor keeps its own guard as
/// defence in depth; with verification refusing first there is no verified
/// grant left to drive it with.)
#[test]
fn an_injection_ref_never_becomes_a_verified_grant() {
    assert!(
        try_install_grant(serde_json::json!({"ref": "org.x; rm -rf /", "remote": "flathub"}))
            .is_err()
    );
    assert!(
        try_install_grant(serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}))
            .is_ok()
    );
}

#[tokio::test]
async fn idempotent_already_installed() {
    let flat = MockFlatpakOps::new().with_installed("org.videolan.VLC");
    let enactor = InstallFlatpakEnactor::new(flat);
    let grant = install_grant(serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}));
    enactor
        .enact(&grant, &charterd::enactor::EnactContext::default())
        .await
        .unwrap();
    assert_eq!(
        enactor.flatpak().install_calls().len(),
        0,
        "already installed -> no-op"
    );
}

#[tokio::test]
async fn request_builder_surfaces_permissions() {
    let flat = MockFlatpakOps::new().with_app(
        "org.videolan.VLC",
        "VLC media player",
        &["network", "audio"],
    );
    let req = build_install_request(&flat, "org.videolan.VLC")
        .await
        .unwrap();
    assert_eq!(req.app_name, "VLC media player");
    assert_eq!(
        req.permissions,
        vec!["network".to_string(), "audio".to_string()]
    );
}

#[tokio::test]
async fn request_builder_unknown_ref_no_request() {
    let flat = MockFlatpakOps::new();
    assert_eq!(
        build_install_request(&flat, "org.unknown.App").await,
        Err(RequestBuildError::NotFound)
    );
}

#[tokio::test]
async fn request_builder_offline_no_request() {
    let flat = MockFlatpakOps::new().with_app("org.videolan.VLC", "VLC", &[]);
    flat.set_offline(true);
    assert_eq!(
        build_install_request(&flat, "org.videolan.VLC").await,
        Err(RequestBuildError::Offline)
    );
}

// --- Full mocked loop -----------------------------------------------------

fn build_loop(
    flat: Arc<MockFlatpakOps>,
) -> (
    Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>,
    TestGuardian,
) {
    let sys = MockSystem::new(NOW);
    let guardian = TestGuardian::new();
    let transport = charterd::MockTransport::new(guardian.pubkey(), PubKey::from_bytes([0x42; 32]));
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(InstallFlatpakEnactor::new(flat)));
    let broker = Broker::new(
        sys,
        transport,
        ScriptedEntropy::new(1),
        reg,
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    (broker, TestGuardian::new())
}

fn ids(
    broker: &Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>,
    hex: &str,
) -> (ReqId, Nonce) {
    let rec = broker.status(hex).pop().unwrap();
    (rec.req_id, rec.nonce)
}

#[tokio::test]
async fn integration_install_golden_path() {
    let flat = Arc::new(MockFlatpakOps::new().with_app("org.videolan.VLC", "VLC", &["network"]));
    let (broker, guardian) = build_loop(flat.clone());
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .params(serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}))
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Enacted);
    assert_eq!(flat.install_calls(), vec!["org.videolan.VLC".to_string()]);
}

#[tokio::test]
async fn integration_install_denied_nothing() {
    let flat = Arc::new(MockFlatpakOps::new().with_app("org.videolan.VLC", "VLC", &[]));
    let (broker, guardian) = build_loop(flat.clone());
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n).deny().build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Denied);
    assert_eq!(flat.install_calls().len(), 0);
}

#[tokio::test]
async fn integration_non_flathub_remote_rejected() {
    let flat = Arc::new(MockFlatpakOps::new());
    let (broker, guardian) = build_loop(flat.clone());
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "x", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    // A grant with an unknown remote fails to parse -> verify Malformed. M6: a
    // malformed/unauthenticated grant is IGNORED (stays Pending) so the genuine
    // grant can still land; the load-bearing property is that nothing installs.
    let grant = GrantBuilder::install_allow(r, n)
        .params(serde_json::json!({"ref": "org.x.Y", "remote": "sketchy"}))
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Pending);
    assert_eq!(flat.install_calls().len(), 0);
}

#[tokio::test]
async fn integration_replay_no_double_install() {
    let flat = Arc::new(MockFlatpakOps::new().with_app("org.videolan.VLC", "VLC", &[]));
    let (broker, guardian) = build_loop(flat.clone());
    let req = broker
        .submit(
            OpType::InstallFlatpak,
            serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}),
            None,
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .params(serde_json::json!({"ref": "org.videolan.VLC", "remote": "flathub"}))
        .build(&guardian);
    broker
        .transport()
        .deliver_grant(grant.clone(), guardian.pubkey());
    broker.poll_once().await;
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(flat.install_calls().len(), 1, "single-use: installed once");
}
