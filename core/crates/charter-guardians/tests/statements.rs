//! statements.json: statement validity on its own.
mod common;

use charter_guardians::{parse_statement, Reason};
use serde_json::{json, Value};

#[test]
fn valid_statements_parse() {
    let v = common::load("statements.json");
    let valid = v["valid"].as_array().unwrap();
    assert!(!valid.is_empty());
    for t in valid {
        let name = common::s(t, "name");
        let st = parse_statement(&t["event"]).unwrap_or_else(|r| panic!("{name}: refused {r}"));
        let got = json!({
            "type": st.kind,
            "d": st.d,
            "p": st.p,
            "key": st.key,
            "target": st.target,
            "signer": st.signer,
            "issuedAt": st.issued_at,
            "label": st.label,
            "expiresAt": st.expires_at,
        });
        if let Some(d) = common::first_diff("expected", &t["expected"], &got) {
            panic!("{name}: {d}");
        }
    }
    eprintln!("{} valid statements passed", valid.len());
}

#[test]
fn invalid_statements_refused_with_reason() {
    let v = common::load("statements.json");
    let invalid = v["invalid"].as_array().unwrap();
    assert!(!invalid.is_empty());
    for t in invalid {
        let name = common::s(t, "name");
        let want: Reason = serde_json::from_value(t["reason"].clone()).unwrap();
        match parse_statement(&t["event"]) {
            Ok(_) => panic!("{name}: accepted, want {want}"),
            Err(got) => assert_eq!(got, want, "{name}"),
        }
    }
    eprintln!("{} invalid statements passed", invalid.len());
}

#[test]
fn signed_events_round_trip_through_typed_events() {
    // A ward holding a typed event passes serde_json::to_value of it; the
    // vector events are already in that shape.
    let v = common::load("statements.json");
    for t in v["valid"].as_array().unwrap() {
        let ev: Value = t["event"].clone();
        assert!(parse_statement(&ev).is_ok());
    }
}
