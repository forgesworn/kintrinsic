//! Golden cross-stack vectors for the `times` clause (§Times). The producer is
//! `scripts/gen-schedule-vectors.mjs` (guardian side of the contract); this
//! side proves the consumer: valid payloads parse and validate, every
//! `invalid` payload fails closed, composition yields the pinned policy, and
//! the additive `time` fields on gifts and time.extend round-trip verbatim.

use charter_content::GrantContent;
use charter_proto::{
    compose_apps, compose_content, GiftBody, GrantApps, GrantTimes, TimeExtendGrantParams,
    TimeExtendRequestParams,
};
use serde_json::Value;

const FILE: &str = "times/times_clause_vectors.json";

#[test]
fn valid_times_clauses_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json(FILE);
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no times vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let g = GrantTimes::from_value(&v["payload"])
            .unwrap_or_else(|e| panic!("{name}: not a valid GrantTimes: {e:?}"));
        let expect = &v["expect"];
        if expect.as_object().is_none_or(|o| o.is_empty()) {
            assert!(g.times.is_empty(), "{name}: expected an empty clause");
            continue;
        }
        let strs = |k: &str| -> Vec<String> {
            expect[k]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: expect.{k}"))
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            g.ids().map(str::to_string).collect::<Vec<_>>(),
            strs("ids"),
            "{name}: ids"
        );
        let runs_nothing: Vec<String> = g
            .times
            .iter()
            .filter(|t| t.runs_nothing())
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(runs_nothing, strs("runsNothing"), "{name}: runsNothing");
        let counted: Vec<String> = g
            .times
            .iter()
            .filter(|t| t.counts_against_budget())
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(counted, strs("countsAgainstBudget"), "{name}: counts");
    }
}

#[test]
fn invalid_times_clauses_fail_closed() {
    let file: Value = charter_testkit::golden::load_json(FILE);
    let invalid = file["invalid"].as_array().expect("invalid array");
    assert!(!invalid.is_empty(), "no invalid vectors");

    for v in invalid {
        let name = v["name"].as_str().unwrap_or("?");
        assert!(
            GrantTimes::from_value(&v["payload"]).is_err(),
            "{name}: must fail closed"
        );
        // The enforcement-side entry point reads "does not validate" as ABSENT.
        assert!(
            GrantTimes::parse_or_absent(&v["payload"].to_string()).is_none(),
            "{name}: must be absent"
        );
    }
}

#[test]
fn composition_matches_the_reference() {
    let file: Value = charter_testkit::golden::load_json(FILE);
    let cases = file["composition"].as_array().expect("composition array");
    assert!(cases.len() >= 10, "suspiciously few composition vectors");

    for c in cases {
        let name = c["name"].as_str().unwrap_or("?");
        let free_apps: GrantApps = serde_json::from_value(c["freeApps"].clone()).unwrap();
        let free_content: Option<GrantContent> = if c["freeContent"].is_null() {
            None
        } else {
            Some(serde_json::from_value(c["freeContent"].clone()).unwrap())
        };
        // The named time is validated through the clause entry point, exactly
        // as a warden would receive it.
        let clause = serde_json::json!({"v": 1, "issuedAt": 1, "times": [c["time"].clone()]});
        let g = GrantTimes::from_value(&clause)
            .unwrap_or_else(|e| panic!("{name}: time does not validate: {e:?}"));
        let t = &g.times[0];

        let want_apps: GrantApps = serde_json::from_value(c["expectApps"].clone()).unwrap();
        assert_eq!(compose_apps(&free_apps, t), want_apps, "{name}: apps");

        let want_content: Option<GrantContent> = if c["expectContent"].is_null() {
            None
        } else {
            Some(serde_json::from_value(c["expectContent"].clone()).unwrap())
        };
        assert_eq!(
            compose_content(free_content.as_ref(), t),
            want_content,
            "{name}: content"
        );
    }
}

#[test]
fn time_fields_on_gifts_and_time_extend_round_trip() {
    let file: Value = charter_testkit::golden::load_json(FILE);
    let params = file["params"].as_array().expect("params array");
    assert!(!params.is_empty(), "no param vectors");

    for p in params {
        let name = p["name"].as_str().unwrap_or("?");
        let want = p["expectTime"].as_str();
        let (time, back) = match p["kind"].as_str().unwrap() {
            "gift" => {
                let g: GiftBody = serde_json::from_value(p["payload"].clone()).unwrap();
                let back = serde_json::to_value(&g).unwrap();
                (g.time, back)
            }
            "time.extend.request" => {
                let r: TimeExtendRequestParams =
                    serde_json::from_value(p["payload"].clone()).unwrap();
                let back = serde_json::to_value(&r).unwrap();
                (r.time, back)
            }
            "time.extend.grant" => {
                let r: TimeExtendGrantParams =
                    serde_json::from_value(p["payload"].clone()).unwrap();
                let back = serde_json::to_value(&r).unwrap();
                (r.time, back)
            }
            other => panic!("{name}: kind {other}"),
        };
        assert_eq!(time.as_deref(), want, "{name}: time");
        // Byte-identical when absent; verbatim when present.
        assert_eq!(back, p["payload"], "{name}: round trip");
    }
}
