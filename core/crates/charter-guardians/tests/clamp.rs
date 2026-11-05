//! The clock clamp's per-slot id quota, which the golden vectors leave out
//! (256 projected steps would bloat scenarios.json). Mirrors the TS test in
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
fn clamped_id_quota_is_256_per_slot() {
    assert_eq!(MAX_REMEMBERED_PER_SLOT, 256);
    let (mut s, w1) = paired();
    let now = T0 + 100;
    for i in 1..=256 {
        let step = clause(&s, &w1, &id(DAY + i), T0 + DAY + i, now);
        assert_eq!(step.outcome.reason, Reason::ClauseAccepted, "clause {i}");
        s = step.state;
    }
    let held = held_clause(&s, "kid", "apps").unwrap();
    assert_eq!(held.remembered.len(), 256);
    assert_eq!(s.clamped, 256);
    let full = clause(&s, &w1, &id(DAY + 257), T0 + DAY + 257, now);
    assert_eq!(full.outcome.reason, Reason::ClampQuota);
    assert!(!full.outcome.accepted());

    // A remembered id is a replay whatever the clock.
    let again = clause(&s, &w1, &id(DAY + 10), T0 + DAY + 10, T0 + DAY + 10);
    assert_eq!(again.outcome.reason, Reason::Replay);

    // A new clamped clause's bar (its effective time) prunes the ids it
    // reaches, freeing room.
    let later = clause(&s, &w1, &id(DAY + 1000), T0 + DAY + 1000, T0 + DAY + 128);
    assert_eq!(later.outcome.reason, Reason::ClauseAccepted);
    let held = held_clause(&later.state, "kid", "apps").unwrap();
    assert_eq!(held.remembered.len(), 129);
    assert_eq!(held.floor, Some(T0 + DAY + 128));

    // A clause that needs no memory prunes every id its bar reaches.
    let settled = clause(
        &later.state,
        &w1,
        &id(DAY + 2000),
        T0 + DAY + 2000,
        T0 + DAY + 3000,
    );
    let held = held_clause(&settled.state, "kid", "apps").unwrap();
    assert!(held.remembered.is_empty());
    let stale = clause(
        &settled.state,
        &w1,
        &id(DAY + 1000),
        T0 + DAY + 1000,
        T0 + DAY + 3001,
    );
    assert_eq!(stale.outcome.reason, Reason::NotNewer);
}

#[test]
fn legacy_state_without_the_clamp_fields_still_reads() {
    let (s, w1) = paired();
    let s = clause(&s, &w1, &id(1), T0 + 5, T0 + 10).state;
    let mut v = serde_json::to_value(&s).unwrap();
    // A state persisted before the clamp: no `clamped`, a clause with no id,
    // no `remembered` and a null floor (its bar is then its issuedAt).
    v.as_object_mut().unwrap().remove("clamped");
    let c = &mut v["clauses"][0];
    let c = c.as_object_mut().unwrap();
    c.remove("id");
    c.remove("remembered");
    c.insert("floor".into(), serde_json::Value::Null);
    let old: GuardianState = serde_json::from_value(v).unwrap();
    assert_eq!(old.clamped, 0);
    let held = held_clause(&old, "kid", "apps").unwrap();
    assert_eq!(held.bar(), T0 + 5);
    assert_eq!(
        clause(&old, &w1, &id(2), T0 + 5, T0 + 20).outcome.reason,
        Reason::NotNewer
    );
    assert_eq!(
        clause(&old, &w1, &id(3), T0 + 6, T0 + 20).outcome.reason,
        Reason::ClauseAccepted
    );
}
