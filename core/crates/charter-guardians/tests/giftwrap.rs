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
