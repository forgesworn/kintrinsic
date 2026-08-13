//! The enforcer wired over the system ports: it locks at the schedule/budget
//! boundary by freezing the managed app.slice + showing the lock + disabling VT
//! switching, thaws on an extension, locks offline from the cached clause, and
//! persists usage across a restart without refilling.

#![cfg(feature = "mock")]

use charter_schedule::{
    Activity, Dimension, GrantBudget, GrantSchedule, GrantScheduleWindow, WeekStart, WeeklySchedule,
};
use charter_sys::persistence::ClauseStore;
use charter_sys::{MockSystem, SystemLayer};
use charterd::enforcer_runtime::{managed_freeze_target, EnforcerRuntime};

const TZ: &str = "Europe/London";
const IN_WINDOW: i64 = 1_782_752_400; // Mon 18:00 BST (16:00-20:00 window open)
const AFTER_WINDOW: i64 = 1_782_766_800; // Mon 22:00 BST (locked)
const MANAGED_UID: u32 = 1000;

/// Decide (sync) + apply the async port effects against the managed slice.
/// The slice is resolved from the managed uid by the caller side now that the
/// spine's `EnforcerRuntime` carries no uid (Linux-only concern).
async fn tick_rt(rt: &mut EnforcerRuntime, sys: &MockSystem, a: Activity, e: u64) {
    let effs = rt.tick(sys, a, e);
    let slice = managed_freeze_target(MANAGED_UID);
    charterd::enforcer_runtime::apply_effects(sys, &effs, &slice).await;
}

fn after_school_json() -> String {
    let s = GrantSchedule {
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
        issued_at: 100,
    };
    serde_json::to_string(&s).unwrap()
}

fn budget_json(daily: u32) -> String {
    let b = GrantBudget {
        model: None,
        v: 1,
        tz: TZ.into(),
        daily_minutes: Some(daily),
        weekly_minutes: None,
        week_start: Some(WeekStart::Mon),
        paused: None,
        revoked: None,
        issued_at: 100,
    };
    serde_json::to_string(&b).unwrap()
}

#[tokio::test]
async fn offline_bedtime_locks_and_freezes_app_slice() {
    let sys = MockSystem::new(AFTER_WINDOW as u64);
    sys.clauses()
        .put_clause(1, 100, &after_school_json())
        .unwrap();

    let mut rt = EnforcerRuntime::new(&sys, TZ);
    // No transport / no network — the cached clause alone locks (fail-safe).
    tick_rt(&mut rt, &sys, Activity::Active, 0).await;

    assert!(rt.is_locked());
    assert!(
        sys.freezer().is_frozen(&managed_freeze_target(MANAGED_UID)),
        "the app.slice must be frozen"
    );
    assert!(sys.session().is_locked(), "the lock must be shown");
    // The lock carries a child-facing reason (not a blank screen).
    let (title, _) = sys.session().last_message().expect("lock message set");
    assert!(
        title == "Outside allowed hours" || title == "Time's up for today",
        "lock title should explain the reason, got {title:?}"
    );
    assert!(
        !sys.vt().switching_enabled(),
        "VT switching must be disabled while locked"
    );
}

#[tokio::test]
async fn budget_lock_then_extension_thaws() {
    let sys = MockSystem::new(IN_WINDOW as u64);
    sys.clauses()
        .put_clause(1, 100, &after_school_json())
        .unwrap();
    sys.clauses().put_clause(2, 100, &budget_json(120)).unwrap();

    let mut rt = EnforcerRuntime::new(&sys, TZ);
    // Burn the whole 120-minute daily quota -> locks on budget.
    tick_rt(&mut rt, &sys, Activity::Active, 120 * 60).await;
    assert!(rt.is_locked());
    assert!(sys.freezer().is_frozen(&managed_freeze_target(MANAGED_UID)));

    // Guardian grants +15 min on the budget dimension -> thaws.
    let applied = rt.apply_extension(IN_WINDOW, "req-extend", 15, Dimension::Budget);
    assert!(applied);
    tick_rt(&mut rt, &sys, Activity::Idle, 0).await; // idle: no further usage
    assert!(!rt.is_locked());
    assert!(
        !sys.freezer().is_frozen(&managed_freeze_target(MANAGED_UID)),
        "thawed"
    );
    assert!(!sys.session().is_locked());
    assert!(sys.vt().switching_enabled(), "VT re-enabled on thaw");
}

#[tokio::test]
async fn usage_persists_across_restart_no_refill() {
    let sys = MockSystem::new(IN_WINDOW as u64);
    sys.clauses().put_clause(2, 100, &budget_json(60)).unwrap(); // 60-min daily cap

    let mut rt = EnforcerRuntime::new(&sys, TZ);
    tick_rt(&mut rt, &sys, Activity::Active, 30 * 60).await; // use 30 of 60 min
    assert!(!rt.is_locked());

    // Restart: a fresh runtime over the same disk reloads usage (no refill).
    let disk = sys.disk();
    drop(rt);
    drop(sys);
    let sys2 = MockSystem::over_disk(IN_WINDOW as u64, disk);
    let mut rt2 = EnforcerRuntime::new(&sys2, TZ);
    // Use the remaining 30 min + 1 more -> should lock (proving 30 already used).
    tick_rt(&mut rt2, &sys2, Activity::Active, 31 * 60).await;
    assert!(rt2.is_locked(), "restart must NOT refill the quota");
}

#[tokio::test]
async fn freeze_targets_managed_uid_not_hardcoded() {
    // M2 regression: the freeze must follow the managed uid, not a literal
    // 1000. Since the spine's EnforcerRuntime carries no uid, the seam under
    // test is the caller resolving the slice via `managed_freeze_target(uid)`
    // and `apply_effects` honoring exactly that slice.
    const OTHER_UID: u32 = 1001;
    let sys = MockSystem::new(AFTER_WINDOW as u64);
    sys.clauses()
        .put_clause(1, 100, &after_school_json())
        .unwrap();
    let mut rt = EnforcerRuntime::new(&sys, TZ);
    let effs = rt.tick(&sys, Activity::Active, 0);
    charterd::enforcer_runtime::apply_effects(&sys, &effs, &managed_freeze_target(OTHER_UID)).await;
    assert!(rt.is_locked());
    assert!(
        sys.freezer().is_frozen(&managed_freeze_target(OTHER_UID)),
        "freeze must target the managed uid's slice"
    );
    assert!(
        !sys.freezer().is_frozen(&managed_freeze_target(1000)),
        "must never freeze a foreign uid's slice"
    );
}

#[tokio::test]
async fn open_window_stays_thawed() {
    let sys = MockSystem::new(IN_WINDOW as u64);
    sys.clauses()
        .put_clause(1, 100, &after_school_json())
        .unwrap();
    let mut rt = EnforcerRuntime::new(&sys, TZ);
    tick_rt(&mut rt, &sys, Activity::Active, 0).await;
    assert!(!rt.is_locked());
    assert!(!sys.freezer().is_frozen(&managed_freeze_target(MANAGED_UID)));
}
