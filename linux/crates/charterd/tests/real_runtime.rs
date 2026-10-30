//! Real-runtime smoke tests (the `real` feature). An *integration* target so it
//! builds the charterd library proper — not its mock-only unit tests — which
//! lets the pure runtime helpers run under `--features real`:
//!
//!   cargo test -p charterd --no-default-features --features real --test real_runtime
//!
//! The live poll/enforce loop itself (`runtime::run`) needs a bus, a relay, a
//! guardian, and privilege, so it is VM-verified — only its pure pieces run here.

#![cfg(feature = "real")]

use charter_primitives::PubKey;
use charter_transport::pairing::Pairing;
use charter_transport::Entropy;
use charterd::runtime::{DaemonConfig, RealEntropy, RealTransportFacade};

#[test]
fn real_entropy_fills_the_whole_buffer_nonzero_and_varies() {
    let e = RealEntropy;
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    e.fill(&mut a);
    e.fill(&mut b);
    assert!(a.iter().any(|&x| x != 0), "must draw randomness");
    assert_ne!(a, b, "two draws must differ (not a fixed fill)");
}

#[test]
fn pairing_json_roundtrips_into_guardian_and_relays() {
    let json = format!(
        r#"{{"guardian_pubkey":"{g}","relays":["wss://relay.example"],"machine":"{m}","subject_pubkey":"{s}","audit_transparency":true,"paired_at":1700000000}}"#,
        g = "ab".repeat(32),
        m = "cd".repeat(32),
        s = "ef".repeat(32),
    );
    let p: Pairing = serde_json::from_str(&json).unwrap();
    assert_eq!(p.guardian_pubkey.to_hex(), "ab".repeat(32));
    assert_eq!(p.relays, vec!["wss://relay.example".to_string()]);
    assert_eq!(p.subject_pubkey.to_hex(), "ef".repeat(32));
}

#[test]
fn default_config_uses_production_paths() {
    let c = DaemonConfig::default();
    assert_eq!(c.key_path, "/var/lib/charter/machine.key");
    assert_eq!(c.managed_uid, 1000);
    assert!(c.poll_interval_secs >= 1);
}

#[test]
fn dbus_op_and_state_mappings_are_total_and_consistent() {
    use charter_ipc::dto::{Op, ReqState};
    use charter_proto::OpType;
    use charterd::dbus_service::{op_from_wire, op_to_ipc, state_to_ipc};
    use charterd::lifecycle::RequestState;

    for (wire, op, ipc) in [
        (
            "install.flatpak",
            OpType::InstallFlatpak,
            Op::InstallFlatpak,
        ),
        ("exec.allow", OpType::ExecAllow, Op::ExecAllow),
        ("time.extend", OpType::TimeExtend, Op::TimeExtend),
        ("app.open", OpType::AppOpen, Op::AppOpen),
    ] {
        assert_eq!(op_from_wire(wire), Some(op));
        assert_eq!(op_to_ipc(op), ipc);
    }
    assert_eq!(op_from_wire("nope"), None);
    assert_eq!(state_to_ipc(RequestState::Pending), ReqState::Pending);
    assert_eq!(state_to_ipc(RequestState::Enacted), ReqState::Enacted);
    assert_eq!(state_to_ipc(RequestState::Rejected), ReqState::Rejected);
}

/// §Times: `SubmitRequest` refuses a `time.extend` whose `time` is not a time
/// this daemon offers the caller; an untyped ask (Free time) always passes.
#[test]
fn submit_request_vets_the_named_time_against_the_caller_s_snapshot() {
    use charter_ipc::dto::{TimeChoice, TimeLeftView, TimesView};
    use charterd::dbus_service::check_requested_time;
    use serde_json::json;

    let mut view = TimeLeftView::unknown();
    view.time = Some(Box::new(TimesView {
        ask: vec![
            TimeChoice {
                id: "free".into(),
                label: "Free time".into(),
            },
            TimeChoice {
                id: "learning".into(),
                label: "Learning time".into(),
            },
        ],
        ..Default::default()
    }));
    let ask =
        |t: serde_json::Value| json!({"minutesRequested": 30, "limitHit": "budget", "time": t});
    let untyped = json!({"minutesRequested": 30, "limitHit": "budget"});

    assert_eq!(check_requested_time(&untyped, Some(&view)), Ok(()));
    assert_eq!(
        check_requested_time(&untyped, None),
        Ok(()),
        "Free needs no snapshot"
    );
    assert_eq!(
        check_requested_time(&ask(json!("learning")), Some(&view)),
        Ok(())
    );
    for bad in [
        json!("chores"),
        json!("free"),
        json!("Learning"),
        json!("x".repeat(33)),
        json!(""),
        json!(42),
        json!(null),
    ] {
        assert!(
            check_requested_time(&ask(bad.clone()), Some(&view)).is_err(),
            "{bad}"
        );
    }
    // No snapshot for the caller: a typed ask cannot be vouched for.
    assert!(check_requested_time(&ask(json!("learning")), None).is_err());
    // A pre-times snapshot offers nothing to name.
    assert!(check_requested_time(&ask(json!("learning")), Some(&TimeLeftView::unknown())).is_err());
}

#[test]
fn record_to_view_projects_without_leaking_hash_or_nonce() {
    use charter_primitives::{Nonce, ReqId};
    use charter_proto::OpType;
    use charterd::dbus_service::record_to_view;
    use charterd::lifecycle::{RequestRecord, RequestState};

    let rec = RequestRecord {
        req_id: ReqId::from_bytes([0xab; 32]),
        nonce: Nonce::from_bytes([0xcd; 32]),
        op: OpType::ExecAllow,
        state: RequestState::Pending,
        created_at: 1700,
        detail: Some("waiting for approval".into()),
        source_path: Some("/home/kid/game.AppImage".into()),
        caller_uid: Some(1000),
    };
    let view = record_to_view(&rec);
    assert_eq!(view.req_id, "ab".repeat(32));
    assert_eq!(view.created_at, 1700);
    // The DTO carries no sha256 / nonce / source_path — only display fields.
    let v = serde_json::to_value(&view).unwrap();
    assert!(v.get("sha256").is_none() && v.get("nonce").is_none());
    assert!(v.get("source_path").is_none());
    // …nor the owning uid (S8). It is what SCOPES the read; publishing it back
    // over the same wire would hand a caller the map of who else is asking.
    assert!(v.get("caller_uid").is_none());
}

#[test]
fn remaining_to_view_makes_next_open_absolute() {
    use charter_schedule::enforcer::Remaining;
    use charterd::dbus_service::remaining_to_view;

    let r = Remaining {
        effective_secs: 600,
        schedule_secs: 600,
        budget_secs: 3600,
        extension_secs: 0,
        locked: false,
        reason: None,
        next_open_secs: Some(120),
        budget_day_secs: 3600,
        budget_week_secs: -1,
    };
    let view = remaining_to_view(&r, 1_000);
    assert_eq!(view.effective_seconds, 600);
    assert!(!view.locked);
    assert!(!view.offline);
    assert_eq!(view.next_open, Some(1_120)); // now + 120

    // Both caps cross the boundary, so a surface can name the one that binds.
    // An unset cap stays -1 rather than becoming a zero that would read as
    // "your week is used up".
    assert_eq!(view.budget_day_seconds, 3600);
    assert_eq!(view.budget_week_seconds, -1);
}

#[test]
fn transport_facade_exposes_machine_and_guardian_pubkeys() {
    // A valid (small) scalar; the facade derives the machine x-only pubkey.
    let mut secret = [0u8; 32];
    secret[31] = 9;
    let guardian = PubKey::from_hex(&"cd".repeat(32)).unwrap();
    let facade = RealTransportFacade::new(secret, guardian, vec!["wss://relay.example".into()]);
    use charterd::transport_facade::TransportFacade as _;
    assert_eq!(facade.pinned_guardian(), guardian);
    let expected_machine = PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret).unwrap());
    assert_eq!(facade.machine_pubkey(), expected_machine);
}

/// A guardian wrap that arrives over the local link opens through the same
/// NIP-59 unwrap as a relay copy and is sorted by inner kind; a wrap that is
/// not addressed to this machine does not open.
#[test]
fn the_real_facade_unwraps_a_local_wrap_exactly_like_a_relay_copy() {
    use charter_primitives::{kinds, EventId, NostrEvent, Sig};
    use charter_transport::nip59::{self, Rumor, WrapRandomness};
    use charterd::transport_facade::{LocalInbound, TransportFacade};

    fn secret(b: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = b;
        s
    }
    let pk = |s: &[u8; 32]| PubKey::from_bytes(charter_crypto::xonly_pubkey(s).unwrap());
    let (machine_sk, guardian_sk, other_sk) = (secret(0x22), secret(0x11), secret(0x33));
    let now = 1_700_000_000;

    let mut clause = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey: pk(&guardian_sk),
        created_at: now,
        kind: kinds::CHARTER_DEVICE_CLAUSE,
        tags: vec![kinds::marker_tag()],
        content: r#"{"v":1,"kind":"schedule"}"#.into(),
        sig: Sig::from_bytes([0; 64]),
    };
    clause.id = charter_verify::event::event_id(&clause);
    clause.sig =
        Sig::from_bytes(charter_crypto::schnorr_sign(&guardian_sk, clause.id.as_bytes()).unwrap());
    let wrap_to = |recipient: &PubKey| {
        nip59::wrap(
            &Rumor::from_signed_event(&clause),
            &guardian_sk,
            recipient.as_bytes(),
            &WrapRandomness {
                ephemeral_secret: secret(0x41),
                seal_nonce: [1; 32],
                wrap_nonce: [2; 32],
                seal_created_at: now,
                wrap_created_at: now,
            },
        )
        .unwrap()
    };

    let facade = RealTransportFacade::new(
        machine_sk,
        pk(&guardian_sk),
        vec!["wss://relay.example".into()],
    );
    match facade.unwrap_local(&wrap_to(&pk(&machine_sk)), now) {
        Some(LocalInbound::Clause(c)) => {
            assert_eq!(c.clause.id, clause.id);
            assert_eq!(c.seal_author, pk(&guardian_sk));
        }
        other => panic!("expected a clause, got {other:?}"),
    }
    assert!(
        facade.unwrap_local(&wrap_to(&pk(&other_sk)), now).is_none(),
        "a wrap for another machine does not open"
    );
}
