//! STATUS `guardians` as the contract shapes it, derived from the golden
//! scenarios' projected state (README: `pending[].id` is `statement`,
//! `pinSet` is `guardianPin` != null, `clamped` as projected).
mod common;

use charter_guardians::*;
use serde_json::{json, Value};

fn final_state(name: &str) -> (GuardianState, Value) {
    let v = common::load("scenarios.json");
    let sc = v["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sc| sc["name"] == name)
        .unwrap_or_else(|| panic!("no scenario {name}"));
    let state = common::replay(sc).unwrap();
    let last = sc["steps"].as_array().unwrap().last().unwrap();
    (state, last["expect"]["state"].clone())
}

#[test]
fn status_guardians_matches_the_projected_state() {
    for name in [
        "statement_clamp",
        "relays_guardian_pin",
        "rule7_removal_by_other_root",
        "clause_clamp",
    ] {
        let (state, projected) = final_state(name);
        let status = serde_json::to_value(status_guardians(&state)).unwrap();
        assert_eq!(status["roots"], projected["roots"], "{name}");
        assert_eq!(status["relays"], projected["relays"], "{name}");
        assert_eq!(status["clamped"], projected["clamped"], "{name}");
        assert_eq!(
            status["pinSet"],
            json!(!projected["guardianPin"].is_null()),
            "{name}"
        );
        let want: Vec<Value> = projected["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut o = json!({
                    "id": p["statement"],
                    "d": p["d"],
                    "kind": p["kind"],
                    "signer": p["signer"],
                    "expiresAt": p["expiresAt"],
                });
                if !p["takesEffectAt"].is_null() {
                    o["takesEffectAt"] = p["takesEffectAt"].clone();
                }
                o
            })
            .collect();
        assert_eq!(status["pending"], Value::Array(want), "{name}");
    }
}

#[test]
fn status_carries_pending_ids_and_signers() {
    let (state, _) = final_state("statement_clamp");
    let status = serde_json::to_value(status_guardians(&state)).unwrap();
    let pending = status["pending"].as_array().unwrap();
    assert!(!pending.is_empty());
    for p in pending {
        assert_eq!(p["id"].as_str().unwrap().len(), 64);
        assert_eq!(p["signer"].as_str().unwrap().len(), 64);
    }
    assert!(status["clamped"].as_u64().unwrap() > 0);
    let (pinned, _) = final_state("relays_guardian_pin");
    assert!(status_guardians(&pinned).pin_set);
}
