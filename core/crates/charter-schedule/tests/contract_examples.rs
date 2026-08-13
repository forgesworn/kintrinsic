//! The shared `budget_vectors.json` (the device-enforced budget arithmetic both
//! repos load) + the `spec/contract.md` GrantBudget example round-trip.

use charter_schedule::{quota_left, Activity, GrantBudget, QuotaStatus, UsageLedger, WeekStart};
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Vectors {
    now_unix: i64,
    vectors: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    budget: GrantBudget,
    #[serde(rename = "usedTodaySecs")]
    used_today_secs: u64,
    expect: Value,
}

#[test]
fn budget_vectors_match() {
    let v: Vectors = charter_testkit::golden::load_json("schedule/budget_vectors.json");
    for case in &v.vectors {
        let mut usage = UsageLedger::new("Europe/London", WeekStart::Mon, v.now_unix);
        usage.credit(v.now_unix, Activity::Active, case.used_today_secs);
        let got = quota_left(&usage, &case.budget, v.now_unix);
        match &case.expect {
            Value::String(s) if s == "unbounded" => {
                assert_eq!(got, QuotaStatus::Unbounded, "{}", case.name);
            }
            Value::Number(n) => {
                assert_eq!(
                    got,
                    QuotaStatus::Remaining(n.as_u64().unwrap()),
                    "{}",
                    case.name
                );
            }
            other => panic!("bad expect for {}: {other:?}", case.name),
        }
    }
}

#[test]
fn grant_budget_roundtrips_spec_json() {
    // The exact shape from spec/contract.md.
    let json = r#"{"v":1,"tz":"Europe/London","dailyMinutes":120,"weeklyMinutes":600,"weekStart":"mon","paused":false,"revoked":false,"issuedAt":1700000000}"#;
    let b: GrantBudget = serde_json::from_str(json).unwrap();
    assert_eq!(b.daily_minutes, Some(120));
    assert_eq!(b.weekly_minutes, Some(600));
    assert_eq!(b.week_start, Some(WeekStart::Mon));
    // Round-trips back.
    let back: GrantBudget = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert_eq!(b, back);
}
