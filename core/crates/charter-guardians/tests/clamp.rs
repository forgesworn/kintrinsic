//! The per-signer replay floors of the clock clamp, beyond what the golden
//! vectors cover: far more clamped events than any old quota, and states
//! persisted before the floors. Mirrors the TS test in
//! `test/guardians/limits.test.ts`.
mod common;

use charter_guardians::*;

const T0: i64 = 1_700_000_000;
const DAY: i64 = 86_400;

/// The end state of the golden scenario `pair_working_key`'s pairing step:
/// R1 pinned, W1 bound.
fn paired() -> (GuardianState, String) {
    let v = common::load("scenarios.json");
    let sc = v["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sc| sc["name"] == "pair_working_key")
        .unwrap();
    let first = &sc["steps"][0]["input"];
    let (state, _) = common::apply_step(None, first);
    let w1 = v["keys"]["W1"]["pubkey"].as_str().unwrap().to_owned();
    (state.unwrap(), w1)
}

fn clause(s: &GuardianState, w: &str, id: &str, issued_at: i64, now: i64) -> Step {
    accept_clause(
        s,
        &ClauseInput {
            id,
            author: w,
            seal_author: w,
            subject: "kid",
            kind: "apps",
            issued_at,
        },
        now,
    )
}

fn id(n: i64) -> String {
    format!("{n:064x}")
}

#[test]
fn clamped_clauses_need_no_quota_one_floor_per_signer() {
    let (mut s, w1) = paired();
    let now = T0 + 100;
    for i in 1..=300 {
        let step = clause(&s, &w1, &id(DAY + i), T0 + DAY + i, now + i);
        assert_eq!(step.outcome.reason, Reason::ClauseAccepted, "clause {i}");
        s = step.state;
    }
    let held = held_clause(&s, "kid", "apps").unwrap();
    assert_eq!(held.signer_floors.len(), 1);
    assert_eq!(held.signer_floors[0].issued_at, T0 + DAY + 300);
    assert_eq!(held.bar(), now + 300);
    assert_eq!(s.clamped, 300);

    // Every superseded one is a replay, whatever the clock; so is a clause
    // whose issuedAt goes backwards.
    for i in [1, 150, 300] {
        let again = clause(&s, &w1, &id(DAY + i), T0 + DAY + i, T0 + DAY + 500);
        assert_eq!(again.outcome.reason, Reason::Replay, "replay {i}");
    }
    assert_eq!(
        clause(&s, &w1, &id(1), now + 1000, now + 1000)
            .outcome
            .reason,
        Reason::Replay
    );
    assert_eq!(
        clause(&s, &w1, &id(DAY + 301), T0 + DAY + 301, now + 400)
            .outcome
            .reason,
        Reason::ClauseAccepted
    );
}

#[test]
fn legacy_state_without_the_clamp_fields_still_reads() {
    let (s, w1) = paired();
    let s = clause(&s, &w1, &id(1), T0 + DAY, T0 + 10).state;
    let mut v = serde_json::to_value(&s).unwrap();
    // A state persisted before the clamp and the floors: no `clamped`, a
    // clause with no id, no `signerFloors`, a null floor (its bar is then its
    // issuedAt), and an old `remembered` list, which is ignored.
    v.as_object_mut().unwrap().remove("clamped");
    v.as_object_mut().unwrap().remove("statementFloors");
    let c = &mut v["clauses"][0];
    let c = c.as_object_mut().unwrap();
    c.remove("id");
    c.remove("signerFloors");
    c.insert("floor".into(), serde_json::Value::Null);
    c.insert(
        "remembered".into(),
        serde_json::json!([{ "id": id(9), "issuedAt": T0 + 2 * DAY }]),
    );
    let old: GuardianState = serde_json::from_value(v).unwrap();
    assert_eq!(old.clamped, 0);
    let held = held_clause(&old, "kid", "apps").unwrap();
    assert_eq!(held.bar(), T0 + DAY);
    assert!(held.signer_floors.is_empty());
    // The held clause is its signer's floor at its real issuedAt.
    assert_eq!(held.floor_of(&w1), Some(T0 + DAY));
    let mut floored = old.clone();
    floored.clauses[0].floor = Some(T0 + 10);
    assert_eq!(
        clause(&floored, &w1, &id(2), T0 + DAY, T0 + 20)
            .outcome
            .reason,
        Reason::Replay
    );
    let next = clause(&floored, &w1, &id(3), T0 + DAY + 1, T0 + 20);
    assert_eq!(next.outcome.reason, Reason::ClauseAccepted);
    // Replaced, it keeps its signer's floor.
    let held = held_clause(&next.state, "kid", "apps").unwrap();
    assert_eq!(held.signer_floors.len(), 1);
    assert_eq!(held.floor_of(&w1), Some(T0 + DAY + 1));
}

/// R1's co-guardian statement naming R2, dated `issued_at`, signed with the
/// golden vectors' test key.
fn coguardian(v: &serde_json::Value, issued_at: i64) -> serde_json::Value {
    let hex32 = |s: &str| -> [u8; 32] {
        let b: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        b.try_into().unwrap()
    };
    let secret = hex32(v["keys"]["R1"]["secret"].as_str().unwrap());
    let r1 = v["keys"]["R1"]["pubkey"].as_str().unwrap();
    let r2 = v["keys"]["R2"]["pubkey"].as_str().unwrap();
    let tags = serde_json::json!([
        ["d", format!("kintrinsic/guardian/{r2}")],
        ["p", r2],
        ["t", "charter-device"]
    ]);
    let content = format!("{{\"v\":1,\"issuedAt\":{issued_at}}}");
    let canon = serde_json::json!([0, r1, issued_at, 31120, tags, content]);
    let hash = charter_crypto::sha256(serde_json::to_string(&canon).unwrap().as_bytes());
    let sig = charter_crypto::schnorr_sign(&secret, &hash).unwrap();
    serde_json::json!({
        "id": event::encode_hex(&hash),
        "pubkey": r1,
        "created_at": issued_at,
        "kind": 31120,
        "tags": tags,
        "content": content,
        "sig": event::encode_hex(&sig),
    })
}

#[test]
fn clamped_statements_need_no_quota_one_floor_per_signer() {
    let v = common::load("scenarios.json");
    let (mut s, w1) = paired();
    let d = format!(
        "kintrinsic/guardian/{}",
        v["keys"]["R2"]["pubkey"].as_str().unwrap()
    );
    let send = |s: &GuardianState, issued_at: i64, now: i64| {
        apply_inbound(s, &w1, &coguardian(&v, issued_at), now)
    };

    // Each arrives a second after the last, so each is newer and supersedes.
    for i in 1..=100 {
        let step = send(&s, T0 + DAY + i, T0 + 100 + i);
        assert_eq!(
            step.outcome.reason,
            Reason::CoguardianPending,
            "statement {i}"
        );
        s = step.state;
    }
    let floors: Vec<_> = s.statement_floors.iter().filter(|f| f.slot == d).collect();
    assert_eq!(floors.len(), 1);
    assert_eq!(floors[0].issued_at, T0 + DAY + 100);

    // The stored one is a duplicate; a superseded far-future one, whose
    // effective time now beats the slot's, is a replay.
    assert_eq!(
        send(&s, T0 + DAY + 100, T0 + 300).outcome.reason,
        Reason::Duplicate
    );
    let replay = send(&s, T0 + DAY + 10, T0 + 300);
    assert_eq!(replay.outcome.reason, Reason::Replay);
    assert_eq!(replay.state.pending[0].issued_at, T0 + DAY + 100);
    // Once the slot's effective time passes it, a replay is merely stale.
    let settled = send(&s, T0 + DAY + 2000, T0 + DAY + 3000).state;
    assert_eq!(
        send(&settled, T0 + DAY + 1000, T0 + DAY + 2500)
            .outcome
            .reason,
        Reason::Stale
    );
}

#[test]
fn legacy_statement_without_floors_still_reads() {
    let v = common::load("scenarios.json");
    let (s, w1) = paired();
    let s = apply_inbound(&s, &w1, &coguardian(&v, T0 + DAY), T0 + 10).state;
    let mut j = serde_json::to_value(&s).unwrap();
    j.as_object_mut().unwrap().remove("statementFloors");
    for st in j["statements"].as_array_mut().unwrap() {
        st.as_object_mut()
            .unwrap()
            .insert("remembered".into(), serde_json::json!([]));
    }
    let old: GuardianState = serde_json::from_value(j).unwrap();
    assert!(old.statement_floors.is_empty());
    // The stored statement is its signer's floor at its real issuedAt.
    assert_eq!(
        apply_inbound(&old, &w1, &coguardian(&v, T0 + 20), T0 + 20)
            .outcome
            .reason,
        Reason::Replay
    );
    let next = apply_inbound(&old, &w1, &coguardian(&v, T0 + DAY + 1), T0 + 20);
    assert_eq!(next.outcome.reason, Reason::CoguardianPending);
}
