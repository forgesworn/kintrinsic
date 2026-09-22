//! The `fail_open_guard_vectors.json` golden set: every case asserts the
//! FAIL-SAFE direction — a clause the device cannot read or cannot understand
//! locks, a `paused` budget is zero on both axes whatever caps it carries, and
//! a DST boundary never shifts a countdown by an hour.
//!
//! These are driven through the **enforcer entry point** (`compute_remaining`),
//! not a library helper. That is the whole point of the file: every pre-existing
//! budget vector drove `quota_left`, which is exactly how a `paused` body with
//! no caps enforced nothing at all in the function the enforcer actually calls
//! (review 2026-09-21, 02-B4) without a single vector noticing.

use charter_schedule::{
    compute_remaining, evaluate_tethering, Activity, EnforcerInputs, ExtensionLedger, GrantBudget,
    GrantSchedule, GrantTethering, LockReason, TetherMode, UsageLedger, WeekStart,
};
use serde_json::Value;

fn reason_str(r: Option<LockReason>) -> &'static str {
    match r {
        None => "none",
        Some(LockReason::Schedule) => "schedule",
        Some(LockReason::Budget) => "budget",
        Some(LockReason::Malformed) => "malformed",
        Some(LockReason::StandDown) => "stand_down",
    }
}

fn mode_str(m: TetherMode) -> &'static str {
    match m {
        TetherMode::Blocked => "blocked",
        TetherMode::Raw => "raw",
        TetherMode::Filtered => "filtered",
    }
}

/// Assert `expect.<key>` when the case carries it. An absent key is "this case
/// does not pin that field", never a silent pass on a wrong one.
fn expect_i64(expect: &Value, key: &str, got: i64, name: &str) {
    if let Some(want) = expect.get(key) {
        let want = want
            .as_i64()
            .unwrap_or_else(|| panic!("{name}: {key} must be an integer"));
        assert_eq!(got, want, "{name}: {key}");
    }
}

#[test]
fn fail_open_guard_vectors_lock() {
    let file: Value = charter_testkit::golden::load_json("schedule/fail_open_guard_vectors.json");
    let cases = file["enforcer"].as_array().expect("enforcer array");
    assert!(!cases.is_empty(), "no enforcer cases");

    for case in cases {
        let name = case["name"].as_str().unwrap_or("?");
        let now = case["nowUnix"].as_i64().expect("nowUnix");
        let tz = case["tz"].as_str().unwrap_or("Etc/UTC");

        let schedule: Option<GrantSchedule> = match case.get("schedule") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value(v.clone())
                    .unwrap_or_else(|e| panic!("{name}: schedule does not parse: {e}")),
            ),
        };
        let budget: Option<GrantBudget> = match case.get("budget") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value(v.clone())
                    .unwrap_or_else(|e| panic!("{name}: budget does not parse: {e}")),
            ),
        };

        let mut usage = UsageLedger::new(tz, WeekStart::Mon, now);
        let used = case["usedTodaySecs"].as_u64().unwrap_or(0);
        if used > 0 {
            usage.credit(now, Activity::Active, used);
        }
        let extension = ExtensionLedger::new(tz, now);

        let rem = compute_remaining(&EnforcerInputs {
            now_unix: now,
            schedule: schedule.as_ref(),
            budget: budget.as_ref(),
            usage: &usage,
            extension: &extension,
            consolidated: None,
            stand_down: None,
        });

        let expect = &case["expect"];
        if let Some(locked) = expect["locked"].as_bool() {
            assert_eq!(rem.locked, locked, "{name}: locked");
        }
        if let Some(want) = expect.get("reason").and_then(|r| r.as_str()) {
            assert_eq!(reason_str(rem.reason), want, "{name}: reason");
        }
        expect_i64(expect, "effectiveSecs", rem.effective_secs, name);
        expect_i64(expect, "scheduleSecs", rem.schedule_secs, name);
        expect_i64(expect, "budgetSecs", rem.budget_secs, name);
        expect_i64(expect, "budgetDaySecs", rem.budget_day_secs, name);
        expect_i64(expect, "budgetWeekSecs", rem.budget_week_secs, name);
        match expect.get("nextOpenSecs") {
            None => {}
            Some(Value::Null) => assert_eq!(rem.next_open_secs, None, "{name}: nextOpenSecs"),
            Some(v) => assert_eq!(
                rem.next_open_secs,
                Some(v.as_i64().expect("nextOpenSecs integer")),
                "{name}: nextOpenSecs"
            ),
        }
    }
}

#[test]
fn fail_open_guard_tethering_vectors_block() {
    let file: Value = charter_testkit::golden::load_json("schedule/fail_open_guard_vectors.json");
    let cases = file["tethering"].as_array().expect("tethering array");
    assert!(!cases.is_empty(), "no tethering cases");

    for case in cases {
        let name = case["name"].as_str().unwrap_or("?");
        let g: GrantTethering = serde_json::from_value(case["payload"].clone())
            .unwrap_or_else(|e| panic!("{name}: payload does not parse: {e}"));
        let now = case["now"].as_u64().expect("now");
        assert_eq!(
            mode_str(evaluate_tethering(Some(&g), now)),
            case["expect"].as_str().expect("expect"),
            "{name}"
        );
    }
}
