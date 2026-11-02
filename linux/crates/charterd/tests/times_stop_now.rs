//! "Stop now" against a named time, composed the way charterd's tick composes
//! it: signed clauses ingested by the broker land in the per-child store, every
//! read below comes back out of that store, and each step runs the enforcer
//! tick, the times layer and `apply_to_decision` in the runtime's own order.
//!
//! Field report: a guardian pressed "Stop now" during a Project time window,
//! re-signed the schedule, and the ward then met the lock screen on logging in
//! inside that window. Both cases are here: with the stand-down in force, and
//! with none in force.

#![cfg(feature = "mock")]

use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_schedule::{Activity, Bucket, Dimension, GrantSchedule, LockReason, WarnLevel};
use charter_spine::child_policy::{EffectivePolicy, PolicySource};
use charter_spine::enforcer_runtime::lock_message;
use charter_spine::multi_child::{ChildDecision, MultiChildEnforcer};
use charter_spine::standdown::{stand_down_now, ChildSlot};
use charter_spine::times::GrantTimes;
use charter_sys::persistence::ChildClauseStore;
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{ClauseBuilder, TestGuardian};
use charterd::lock_info::lock_info;
use charterd::ports::NullEventSink;
use charterd::times_routing::{
    apply_to_decision, charge_activity, following_label, lock_copy, state_for, times_view,
    warning_text, TimesState, Verdict, WarnTracker,
};
use charterd::{Broker, EnactorRegistry, MockTransport};
use serde_json::{json, Value};

const WARD: PubKey = PubKey::from_bytes([0xA1; 32]);
const UID: u32 = 1001;
/// Mon 2026-06-29 00:00:00 UTC.
const MON: u64 = 1_782_691_200;

fn at(hh: u64, mm: u64) -> u64 {
    MON + hh * 3600 + mm * 60
}

type B = Broker<MockSystem, MockTransport, ScriptedEntropy>;

fn broker(g: &TestGuardian) -> B {
    Broker::new(
        MockSystem::new(at(19, 0)),
        MockTransport::new(g.pubkey(), PubKey::from_bytes([0x42; 32])),
        ScriptedEntropy::new(1),
        EnactorRegistry::new(),
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    )
}

async fn deliver(b: &B, g: &TestGuardian, c: ClauseBuilder) {
    b.transport()
        .deliver_clause(c.subject(WARD).build(g), g.pubkey());
    b.poll_once().await;
}

fn schedule_clause(issued_at: u64, day: Value) -> ClauseBuilder {
    ClauseBuilder::schedule(issued_at).body(json!({
        "v": 1, "tz": "Etc/UTC", "issuedAt": issued_at,
        "weekly": { "mon": day, "tue": day }
    }))
}

fn overlapping() -> Value {
    json!([
        {"start": "16:00", "end": "20:00"},
        {"start": "07:00", "end": "20:15", "time": "learning"}
    ])
}

fn project_only() -> Value {
    json!([{"start": "07:00", "end": "20:15", "time": "learning"}])
}

fn times_clause(issued_at: u64) -> ClauseBuilder {
    ClauseBuilder {
        kind: ClauseKind::Times,
        issued_at,
        body: json!({
            "v": 1, "issuedAt": issued_at,
            "times": [{"id": "learning", "label": "Project time", "posture": "onlyThese",
                       "apps": ["/usr/bin/gimp"]}]
        }),
        created_at: issued_at,
        subject: None,
    }
}

/// One pass of charterd's tick for this ward, active at the display.
fn tick(b: &B, multi: &mut MultiChildEnforcer, now: u64) -> (TimesState, ChildDecision) {
    let store = b.sys().child_clauses();
    let subject = WARD.to_hex();
    let read = |kind: ClauseKind| store.get_child_clause(&subject, kind.store_key()).unwrap();
    let schedule: GrantSchedule =
        serde_json::from_str(&read(ClauseKind::Schedule).expect("schedule stored")).unwrap();
    let times = read(ClauseKind::Times).and_then(|j| GrantTimes::parse_or_absent(&j));
    multi.sync(
        &[(
            UID,
            EffectivePolicy {
                schedule: Some(schedule.clone()),
                budget: None,
                learning: None,
                source: PolicySource::Guardian,
            },
        )],
        now as i64,
        |_| (None, None),
    );
    let sd = stand_down_now(
        &ChildSlot {
            store,
            subject_hex: &subject,
        },
        now,
    );
    let secs = sd.as_ref().map(|s| s.secs_until_lock);
    multi.set_stand_down(UID, sd);
    let before = state_for(
        multi,
        Some(&schedule),
        None,
        times.as_ref(),
        true,
        secs,
        UID,
        now as i64,
    )
    .unwrap();
    let charge = charge_activity(Activity::Active, &before.verdict);
    let mut ds = multi.tick_attributed_with(
        charge,
        Some(UID),
        now as i64,
        2,
        Bucket::Screen,
        None,
        false,
    );
    let st = state_for(
        multi,
        Some(&schedule),
        None,
        times.as_ref(),
        true,
        secs,
        UID,
        now as i64,
    )
    .unwrap();
    let mut d = ds.remove(0);
    apply_to_decision(&mut d, &st);
    (st, d)
}

fn is_project(st: &TimesState) -> bool {
    matches!(&st.verdict, Verdict::Named { id, .. } if id == "learning")
}

async fn seeded(g: &TestGuardian, day: Value) -> B {
    let b = broker(g);
    deliver(&b, g, times_clause(at(19, 0))).await;
    deliver(&b, g, schedule_clause(at(19, 0), day)).await;
    b
}

/// CASE 2, no stand-down in force: nothing locks a login inside the window.
#[tokio::test]
async fn with_no_stand_down_a_login_in_the_project_window_opens() {
    let g = TestGuardian::new();
    for day in [overlapping(), project_only()] {
        let b = seeded(&g, day).await;
        let mut multi = MultiChildEnforcer::new();
        for now in [at(19, 59), at(20, 1), at(20, 14)] {
            let (st, d) = tick(&b, &mut multi, now);
            assert!(!d.locked, "{now}: {d:?}");
            assert!(is_project(&st));
        }
        let (_, d) = tick(&b, &mut multi, at(20, 16));
        assert!(d.locked, "20:16 is outside every window");
    }
}

/// CASE 1, the timeline as reported. A bare 60 s warning that reads as Project
/// time ending, then a lock that re-signing the schedule and giving time do not
/// touch. Only "Allow back on" does.
#[tokio::test]
async fn stop_now_locks_the_named_time_and_outlasts_a_new_schedule_and_a_gift() {
    let g = TestGuardian::new();
    let b = seeded(&g, overlapping()).await;
    let mut multi = MultiChildEnforcer::new();
    let t0 = at(19, 58);
    let (st, d) = tick(&b, &mut multi, t0);
    assert!(is_project(&st) && !d.locked);

    // "Stop now", as signStandDownClause signs it: expiry at the ward's
    // midnight, 60 s of grace, no device and no time in the shape.
    deliver(&b, &g, ClauseBuilder::stand_down(t0, "stop-1", at(24, 0))).await;

    // The grace: still Project time, capped, and the warning says so.
    let (st, d) = tick(&b, &mut multi, t0);
    assert!(is_project(&st) && !d.locked);
    assert_eq!(st.remaining.effective_secs, 60);
    let store = b.sys().child_clauses();
    let raw = store
        .get_child_clause(&WARD.to_hex(), ClauseKind::Schedule.store_key())
        .unwrap()
        .unwrap();
    let schedule: GrantSchedule = serde_json::from_str(&raw).unwrap();
    let times = GrantTimes::parse_or_absent(
        &store
            .get_child_clause(&WARD.to_hex(), ClauseKind::Times.store_key())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let then = following_label(&st, Some(&schedule), Some(&times));
    assert_eq!(then, None);
    let mut tracker = WarnTracker::default();
    let level = tracker.observe("learning", st.remaining.effective_secs);
    assert_eq!(level, Some(WarnLevel::One));
    assert_eq!(
        warning_text(WarnLevel::One, "Project time", None, 60).1,
        "1 minute left of Project time — then locked. Save your game."
    );

    // Past the grace: a locked NAMED window, in the core's verdict, level-held.
    let (st, d) = tick(&b, &mut multi, t0 + 61);
    assert!(d.locked);
    assert_eq!(d.reason, Some(LockReason::StandDown));
    assert_eq!(st.verdict, Verdict::Core { span_end: None });

    // Re-signing the schedule is a different slot: the lock does not move.
    deliver(&b, &g, schedule_clause(at(20, 2), project_only())).await;
    let (_, d) = tick(&b, &mut multi, at(20, 3));
    assert_eq!(d.reason, Some(LockReason::StandDown), "new schedule: {d:?}");

    // His ask, answered by the guardian either way: a typed grant, and Free.
    assert!(multi.apply_time_extension(UID, at(20, 3) as i64, "ask-1", 30, "learning"));
    assert!(multi.apply_extension(UID, at(20, 3) as i64, "gift-1", 30, Dimension::Schedule));
    assert!(multi.apply_extension(UID, at(20, 3) as i64, "gift-2", 30, Dimension::Budget));
    let (_, d) = tick(&b, &mut multi, at(20, 4));
    assert_eq!(d.reason, Some(LockReason::StandDown), "gifts: {d:?}");

    // "Allow back on": the same clause kind, already expired at issue.
    let lift = at(20, 5);
    deliver(
        &b,
        &g,
        ClauseBuilder::stand_down(lift, "stop-lift", lift - 86_400),
    )
    .await;
    let (st, d) = tick(&b, &mut multi, lift);
    assert!(!d.locked, "{d:?}");
    assert!(is_project(&st));
}

/// It does not outlive the day. Left alone it lapses at the ward's midnight and
/// the next login inside the window opens.
#[tokio::test]
async fn stop_now_lapses_at_midnight_with_nothing_left_behind() {
    let g = TestGuardian::new();
    let b = seeded(&g, project_only()).await;
    let mut multi = MultiChildEnforcer::new();
    deliver(
        &b,
        &g,
        ClauseBuilder::stand_down(at(19, 58), "stop-1", at(24, 0)),
    )
    .await;
    tick(&b, &mut multi, at(19, 58));
    let (_, d) = tick(&b, &mut multi, at(23, 59));
    assert_eq!(d.reason, Some(LockReason::StandDown));
    // Tuesday 07:05.
    let (st, d) = tick(&b, &mut multi, at(24 + 7, 5));
    assert!(!d.locked, "the next morning must open: {d:?}");
    assert!(is_project(&st));
}

/// What the lock card is built from while it stands, in `apply_child_decisions`'
/// order: the ordinary stand-down copy, nothing from the times layer, the week's
/// windows with no tags. `times_view` still computes the typed-ask choices;
/// `apply_child_decisions` drops the ask for a stand-down (`is_stand_down`).
#[tokio::test]
async fn the_lock_card_while_it_stands() {
    let g = TestGuardian::new();
    let b = seeded(&g, project_only()).await;
    let mut multi = MultiChildEnforcer::new();
    deliver(
        &b,
        &g,
        ClauseBuilder::stand_down(at(19, 58), "stop-1", at(24, 0)),
    )
    .await;
    tick(&b, &mut multi, at(19, 58));
    let now = at(20, 5);
    let (st, d) = tick(&b, &mut multi, now);
    assert!(d.locked);

    let store = b.sys().child_clauses();
    let schedule: GrantSchedule = serde_json::from_str(
        &store
            .get_child_clause(&WARD.to_hex(), ClauseKind::Schedule.store_key())
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let times = GrantTimes::parse_or_absent(
        &store
            .get_child_clause(&WARD.to_hex(), ClauseKind::Times.store_key())
            .unwrap()
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        lock_message(LockReason::StandDown),
        (
            "That's it for today",
            "Your guardian asked you to finish up for today. It ends at midnight, or sooner if they let you back on."
        )
    );
    let info = lock_info(
        Some(&schedule),
        None,
        st.remaining.next_open_secs,
        0,
        now as i64,
    );
    assert_eq!(
        info.title, None,
        "the stand-down headline is not overridden"
    );
    assert_eq!(info.detail, None, "and no 'come back at' line either");
    assert!(
        info.lines.iter().any(|l| l.contains("07:00 – 20:15")),
        "the week's hours are listed: {:?}",
        info.lines
    );
    assert_eq!(
        lock_copy(&st, Some(&schedule), Some(&times), now as i64),
        None
    );

    let v = times_view(
        &st,
        Some(&schedule),
        Some(&times),
        &[],
        chrono_tz::Etc::UTC,
        now as i64,
    )
    .unwrap();
    assert_eq!(v.ask_default.as_deref(), Some("learning"));
    assert!(v.ask.iter().any(|c| c.label == "Project time"));
    assert_eq!(v.lock_title, None);
}
