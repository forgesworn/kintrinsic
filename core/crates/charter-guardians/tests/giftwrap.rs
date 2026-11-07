//! giftwrap.json: full NIP-59 wraps addressed to the ward, unwrapped with the
//! committed NIP-44 v2 primitives, then applied as an `inbound` step on the
//! end state of the setup scenario.
mod common;

use charter_guardians::event::{decode_hex_vec, verified_event};
use charter_transport::nip44;
use serde_json::Value;

fn key32(hex: &str) -> [u8; 32] {
    decode_hex_vec(hex).unwrap().try_into().unwrap()
}

/// Open one NIP-44 layer: returns the inner JSON.
fn open(layer: &Value, recipient_sk: &[u8; 32]) -> Value {
    let ck = nip44::conversation_key(recipient_sk, &key32(common::s(layer, "pubkey"))).unwrap();
    let plain = nip44::decrypt(common::s(layer, "content"), &ck).unwrap();
    serde_json::from_str(&plain).unwrap()
}

#[test]
fn every_giftwrap_unwraps_and_applies() {
    let v = common::load("giftwrap.json");
    let sk = key32(common::s(&v, "recipient_secret"));
    let scenarios = common::load("scenarios.json");
    let vectors = v["vectors"].as_array().unwrap();
    assert!(!vectors.is_empty());
    for t in vectors {
        let name = common::s(t, "name");
        let wrap = &t["wrap"];
        assert_eq!(wrap["kind"], 1059, "{name}");
        assert!(verified_event(wrap).is_some(), "{name}: wrap signature");
        let seal = open(wrap, &sk);
        assert_eq!(seal["kind"], 13, "{name}");
        let seal_ev = verified_event(&seal).unwrap_or_else(|| panic!("{name}: seal signature"));
        assert_eq!(
            seal_ev.pubkey,
            common::s(t, "expected_seal_author"),
            "{name}"
        );
        let inner = open(&seal, &sk);
        assert_eq!(inner, t["expected_inner"], "{name}: inner");

        let setup = common::s(t, "setup");
        let sc = scenarios["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sc| sc["name"] == setup)
            .unwrap_or_else(|| panic!("{name}: no setup scenario {setup}"));
        let state = common::replay(sc).expect("setup paired");
        let input = serde_json::json!({
            "type": "inbound",
            "now": t["now"],
            "sealAuthor": seal_ev.pubkey,
            "inner": inner,
        });
        let (_, got) = common::apply_step(Some(&state), &input);
        if let Some(d) = common::first_diff("expect", &t["expect"], &got) {
            panic!("{name}: {d}");
        }
    }
    eprintln!("{} giftwraps passed", vectors.len());
}

/// The ward's own path: the transport's seal-exempt unwrap
/// (`nip59::unwrap_opened`) hands a statement over as `GuardianSealed`, whose
/// typed event, turned back into JSON, is what `apply_inbound` reads. Each
/// vector must come out with the same inner event, seal author and verdict
/// as the layer-by-layer open above.
#[test]
fn every_giftwrap_applies_through_unwrap_opened() {
    use charter_primitives::NostrEvent;
    use charter_transport::nip59::{unwrap_opened, Opened, MAX_JITTER_SECS};

    let v = common::load("giftwrap.json");
    let sk = key32(common::s(&v, "recipient_secret"));
    let scenarios = common::load("scenarios.json");
    for t in v["vectors"].as_array().unwrap() {
        let name = common::s(t, "name");
        let wrap: NostrEvent = serde_json::from_value(t["wrap"].clone()).unwrap();
        let now = t["now"].as_u64().unwrap();
        let sealed = match unwrap_opened(&wrap, &sk, now, MAX_JITTER_SECS) {
            Ok(Opened::Guardian(sealed)) => sealed,
            other => panic!("{name}: expected a seal-exempt statement, got {other:?}"),
        };
        assert_eq!(
            sealed.seal_author().to_hex(),
            common::s(t, "expected_seal_author"),
            "{name}"
        );
        let inner = serde_json::to_value(sealed.event()).unwrap();
        assert_eq!(inner, t["expected_inner"], "{name}: typed event round trip");

        let setup = common::s(t, "setup");
        let sc = scenarios["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sc| sc["name"] == setup)
            .unwrap();
        let state = common::replay(sc).expect("setup paired");
        let step = charter_guardians::apply_inbound(
            &state,
            &sealed.seal_author().to_hex(),
            &inner,
            now as i64,
        );
        let input = serde_json::json!({
            "type": "inbound",
            "now": t["now"],
            "sealAuthor": sealed.seal_author().to_hex(),
            "inner": inner,
        });
        let (_, got) = common::apply_step(Some(&state), &input);
        if let Some(d) = common::first_diff("expect", &t["expect"], &got) {
            panic!("{name}: {d}");
        }
        assert_eq!(
            step.outcome.accepted(),
            got["result"] == "accepted",
            "{name}: direct call agrees with the harness"
        );
    }
}
