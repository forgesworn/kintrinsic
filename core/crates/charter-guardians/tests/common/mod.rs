//! Shared loaders and the scenario replayer for the golden vectors.
#![allow(dead_code)]

use charter_guardians::*;
use serde_json::{json, Value};

/// Load `charter-testkit/vectors/guardians/<name>`, refusing any version but 1.
pub fn load(name: &str) -> Value {
    let path = format!(
        "{}/../charter-testkit/vectors/guardians/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(v["version"], json!(1), "{name}: version must be 1");
    v
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str()
        .unwrap_or_else(|| panic!("{k} not a string in {v}"))
}

pub fn i(v: &Value, k: &str) -> i64 {
    v[k].as_i64()
        .unwrap_or_else(|| panic!("{k} not an integer in {v}"))
}

/// The first path at which two JSON values differ.
pub fn first_diff(path: &str, want: &Value, got: &Value) -> Option<String> {
    match (want, got) {
        (Value::Object(a), Value::Object(b)) => {
            for k in a.keys().chain(b.keys()) {
                let (x, y) = (
                    a.get(k).unwrap_or(&Value::Null),
                    b.get(k).unwrap_or(&Value::Null),
                );
                if a.contains_key(k) != b.contains_key(k) {
                    return Some(format!("{path}.{k}: want {x}, got {y} (member presence)"));
                }
                if let Some(d) = first_diff(&format!("{path}.{k}"), x, y) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .find_map(|(n, (x, y))| first_diff(&format!("{path}[{n}]"), x, y)),
        _ if want == got => None,
        _ => Some(format!("{path}: want {want}, got {got}")),
    }
}

fn outcome_json(o: &Outcome, state: Option<&GuardianState>, now: i64) -> Value {
    json!({
        "result": o.result,
        "reason": o.reason,
        "ticked": o.ticked,
        "departed": o.departed,
        "state": state.map(|st| project_state(st, now)),
    })
}

/// Apply one scenario step; returns the new state and the step's expect JSON.
pub fn apply_step(state: Option<&GuardianState>, input: &Value) -> (Option<GuardianState>, Value) {
    let now = i(input, "now");
    let ty = s(input, "type");
    if ty == "pair_offer" {
        let tok = &input["token"];
        let token = PairToken {
            token: s(tok, "token"),
            minted_at: i(tok, "mintedAt"),
        };
        let r = pair_from_offer(s(input, "sealAuthor"), &input["inner"], &token, now);
        let (state, result, reason) = match r {
            Ok(p) => (Some(p.state), Verdict::Accepted, Reason::Paired),
            Err(reason) => (state.cloned(), Verdict::Dropped, reason),
        };
        let o = Outcome {
            result,
            reason,
            ticked: vec![],
            departed: vec![],
        };
        let e = outcome_json(&o, state.as_ref(), now);
        return (state, e);
    }
    let st = state.expect("step before pairing");
    let step = match ty {
        "inbound" => apply_inbound(st, s(input, "sealAuthor"), &input["inner"], now),
        "tick" => tick(st, now),
        "presence_token" => {
            register_presence_token(st, s(input, "token"), input["statementD"].as_str(), now)
        }
        "presence_confirm" => {
            apply_presence_confirm(st, s(input, "sealAuthor"), &input["inner"], now)
        }
        "clause" => accept_clause(
            st,
            &ClauseInput {
                author: s(input, "author"),
                seal_author: s(input, "sealAuthor"),
                subject: s(input, "subject"),
                kind: s(input, "kind"),
                issued_at: i(input, "issuedAt"),
            },
            now,
        ),
        "signed" => accept_signed_event(
            st,
            s(input, "author"),
            s(input, "sealAuthor"),
            i(input, "ts"),
            now,
        ),
        "relays" => {
            let relays: Vec<String> = serde_json::from_value(input["relays"].clone()).unwrap();
            let channel = match s(input, "channel") {
                "wrapped" => RelaysChannel::Wrapped {
                    seal_author: s(input, "sealAuthor"),
                },
                "pasted" => RelaysChannel::Pasted,
                c => panic!("unknown channel {c}"),
            };
            apply_relays(
                st,
                &RelaysInput {
                    author: s(input, "author"),
                    channel,
                    ts: i(input, "ts"),
                    relays: &relays,
                    guardian_pin: input.get("guardianPin"),
                },
                now,
            )
        }
        "status_sent" => mark_status_sent(st, now),
        other => panic!("unknown input type {other}"),
    };
    let e = outcome_json(&step.outcome, Some(&step.state), now);
    (Some(step.state), e)
}

/// Replay a whole scenario, checking every step. Returns the final state.
pub fn replay(sc: &Value) -> Option<GuardianState> {
    let name = s(sc, "name");
    let mut state: Option<GuardianState> = None;
    for (n, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
        let input = &step["input"];
        let (next, got) = apply_step(state.as_ref(), input);
        if let Some(d) = first_diff("expect", &step["expect"], &got) {
            panic!("scenario {name}, step {n} ({}): {d}", s(input, "type"));
        }
        // The persisted state round-trips through serde.
        if let Some(st) = &next {
            let back: GuardianState =
                serde_json::from_value(serde_json::to_value(st).unwrap()).unwrap();
            assert_eq!(&back, st, "scenario {name}, step {n}: state round-trip");
        }
        state = next;
    }
    state
}
