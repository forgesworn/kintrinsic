//! Giving and taking back time AT THE COMPUTER, end to end through a real
//! enforcer — the standalone path (`charter-time` -> file -> loop -> ledger).
//!
//! The loop itself is glue; this drives the same [`local_adjust::apply_to`] the
//! loop calls, against a live `MultiChildEnforcer`, so the parts that actually
//! decide a ward's day are covered: routing, idempotency across ticks, the
//! day roll, and the give/take cancellation.

use charter_schedule::clause::{GrantBudget, GrantSchedule, GrantScheduleWindow, WeeklySchedule};
use charter_schedule::WeekStart;
use charter_spine::child_policy::{EffectivePolicy, PolicySource};
use charter_spine::multi_child::MultiChildEnforcer;
use charterd::local_adjust::{apply_to, AdjustEntry, AdjustRecord, ADJUST_VERSION};

const TZ: &str = "Europe/London";
/// Mon 2026-06-29 18:00 BST — inside the 16:00–20:00 window below.
const IN_WINDOW: i64 = 1_782_752_400;
const UID: u32 = 1002;

fn budget(daily: u32) -> GrantBudget {
    GrantBudget {
        model: None,
        v: 1,
        tz: TZ.into(),
        daily_minutes: Some(daily),
        weekly_minutes: None,
        week_start: Some(WeekStart::Mon),
        paused: None,
        revoked: None,
        issued_at: 1,
    }
}

fn after_school() -> GrantSchedule {
    GrantSchedule {
        v: 1,
        tz: TZ.into(),
        paused: None,
        weekly: WeeklySchedule {
            mon: Some(vec![GrantScheduleWindow {
                start: "16:00".into(),
                end: "20:00".into(),
            }]),
            ..Default::default()
        },
        overrides: None,
        issued_at: 1,
    }
}

fn enforcer(policy: EffectivePolicy) -> MultiChildEnforcer {
    let mut multi = MultiChildEnforcer::new();
    multi.sync(&[(UID, policy)], IN_WINDOW, |_| None);
    multi
}

fn policy(schedule: Option<GrantSchedule>, budget: Option<GrantBudget>) -> EffectivePolicy {
    EffectivePolicy {
        schedule,
        budget,
        learning: None,
        source: PolicySource::DeviceOnly,
    }
}

fn record(entries: &[(&str, i32)]) -> AdjustRecord {
    AdjustRecord {
        v: ADJUST_VERSION,
        entries: entries
            .iter()
            .map(|(id, minutes)| AdjustEntry {
                id: (*id).into(),
                minutes: *minutes,
                at: IN_WINDOW,
                by: Some("decented".into()),
            })
            .collect(),
    }
}

/// The headline: a guardian at the keyboard takes twenty minutes off, and the
/// ward's remaining time drops by exactly twenty.
#[test]
fn a_take_back_at_the_computer_shortens_the_day() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    let before = multi.remaining(UID, IN_WINDOW).unwrap().budget_secs;
    assert_eq!(before, 120 * 60);

    let net = apply_to(&mut multi, UID, IN_WINDOW, &record(&[("a", -20)]));
    assert_eq!(net, -20);
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        100 * 60
    );
}

/// And a give adds exactly what it says.
#[test]
fn a_give_at_the_computer_lengthens_the_day() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    let net = apply_to(&mut multi, UID, IN_WINDOW, &record(&[("a", 30)]));
    assert_eq!(net, 30);
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        150 * 60
    );
}

/// The loop re-reads the file EVERY tick. Without idempotency a "+30" would
/// become +30 a second, which is the whole reason entries carry ids.
#[test]
fn re_reading_the_file_every_tick_applies_each_entry_once() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    let r = record(&[("a", 30), ("b", -10)]);

    let first = apply_to(&mut multi, UID, IN_WINDOW, &r);
    assert_eq!(first, 20);
    let after_first = multi.remaining(UID, IN_WINDOW).unwrap().budget_secs;

    // Ten more ticks, same file.
    for _ in 0..10 {
        assert_eq!(apply_to(&mut multi, UID, IN_WINDOW, &r), 0);
    }
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        after_first
    );
    assert_eq!(after_first, 140 * 60);
}

/// A give and an equal take-back leave the day exactly as it was.
#[test]
fn a_give_and_an_equal_take_back_cancel_exactly() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    apply_to(
        &mut multi,
        UID,
        IN_WINDOW,
        &record(&[("a", 30), ("b", -30)]),
    );
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        120 * 60
    );
}

/// When a closed-ish WINDOW is the binding wall, the minutes must move the
/// window — minutes routed to the budget pool would change nothing the ward
/// can see. Here the window has 2h left and the budget 8h, so the window binds.
#[test]
fn minutes_land_on_the_wall_the_ward_is_actually_up_against() {
    let mut multi = enforcer(policy(Some(after_school()), Some(budget(480))));
    let before = multi.remaining(UID, IN_WINDOW).unwrap();
    assert_eq!(before.schedule_secs, 2 * 3600);
    assert_eq!(before.effective_secs, 2 * 3600); // the window is what binds

    apply_to(&mut multi, UID, IN_WINDOW, &record(&[("a", -30)]));
    let after = multi.remaining(UID, IN_WINDOW).unwrap();
    assert_eq!(after.schedule_secs, 90 * 60, "the window moved");
    assert_eq!(after.budget_secs, 480 * 60, "the budget did not");
    assert_eq!(after.effective_secs, 90 * 60);
}

/// Taking back more than remains locks the ward, and reads as an ordinary
/// spent budget — which is honest: the allowance is gone and the way out is
/// tomorrow, exactly as if they had used it.
#[test]
fn taking_back_everything_locks_the_ward_on_the_budget() {
    let mut multi = enforcer(policy(None, Some(budget(30))));
    apply_to(&mut multi, UID, IN_WINDOW, &record(&[("a", -60)]));
    let r = multi.remaining(UID, IN_WINDOW).unwrap();
    assert!(r.locked);
    assert_eq!(r.budget_secs, 0);
}

/// Today-only, like every other extension: tomorrow starts clean, so a
/// take-back can never become a durable cut nobody remembers making.
#[test]
fn an_adjustment_does_not_survive_the_day_roll() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    apply_to(&mut multi, UID, IN_WINDOW, &record(&[("a", -60)]));
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        60 * 60
    );

    let tomorrow = IN_WINDOW + 24 * 3600;
    assert_eq!(
        multi.remaining(UID, tomorrow).unwrap().budget_secs,
        120 * 60,
        "yesterday's take-back must not follow the ward into today"
    );
}

/// A ward this machine does not track absorbs nothing — no panic, no phantom
/// grant waiting to land on whoever gets that uid next.
#[test]
fn an_untracked_uid_is_a_no_op() {
    let mut multi = enforcer(policy(None, Some(budget(120))));
    assert_eq!(
        apply_to(&mut multi, 4242, IN_WINDOW, &record(&[("a", 30)])),
        0
    );
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        120 * 60
    );
}

/// A record that survived a round trip through the helper's own file format
/// applies identically — the on-disk shape and the in-memory one cannot drift.
#[test]
fn a_record_read_back_from_its_json_applies_the_same() {
    let r = record(&[("a", 45), ("b", -15)]);
    let round_tripped = AdjustRecord::parse(&serde_json::to_string(&r).unwrap());
    assert_eq!(round_tripped, r);

    let mut multi = enforcer(policy(None, Some(budget(120))));
    assert_eq!(apply_to(&mut multi, UID, IN_WINDOW, &round_tripped), 30);
    assert_eq!(
        multi.remaining(UID, IN_WINDOW).unwrap().budget_secs,
        150 * 60
    );
}
