//! Times (§Times) on the Android ward: which TIME the device is in now, whether
//! it may run, and what the ward is told about it.
//!
//! The shared crates own the pieces (`charter_schedule::times` evaluates tagged
//! windows, `charter_proto::times` composes a named time over Free time, the
//! `ExtensionLedger` stores the per-time pools). This module is the one place
//! the Android warden joins them to the lock math, so the five readers of "is
//! the device locked?" (the tick, `time_left`, `lock_info`, STATUS and the home
//! view) cannot disagree about it.
//!
//! # Why not `compute_remaining`
//!
//! `compute_remaining` still evaluates the schedule with the CLASSIC evaluator,
//! which reads `start`/`end` only and so runs a tagged window as Free time. For
//! an unknown tag that is the fail-OPEN direction, and it cannot express "the
//! named pool carries the device as Learning time". [`assess`] therefore
//! recomputes the schedule dimension from [`charter_schedule::evaluate_times`]
//! and reuses `compute_remaining` only for the BUDGET dimension (the quota
//! arithmetic is not duplicated). For a schedule with no tagged window the
//! schedule dimension is derived from the classic evaluator, so a ward whose
//! guardian never uses times computes bit-for-bit what it always did (pinned
//! by `an_untagged_schedule_assesses_exactly_as_compute_remaining`).
//!
//! # Fall-through (the contract's order)
//!
//! Inside a window that can run, the window decides and no pool burns. When
//! the window's time cannot run, the device falls through: Free's pool for the
//! dimension that is locking (folded in below, exactly today's routing), then
//! the named pool most recently topped up among the times THIS device defines,
//! run AS that time; nothing left means locked. A `Locked` result from the
//! times evaluator, an unknown tag included, always closes the schedule
//! dimension; it is never run as Free.

use charter_proto::{GrantTimes, NamedTime, TimePosture};
use charter_schedule::enforcer::{
    compute_remaining, enforcement_eod_unix, AuditKind, EnforcerEffect, EnforcerInputs,
    FreezeTarget, LockReason, Remaining, StandDown, WarnLevel,
};
use charter_schedule::{
    evaluate_grant_schedule, evaluate_times, next_window, ConsolidatedUsage, ExtensionLedger,
    GrantBudget, GrantSchedule, LockCause, NextWindow, ScheduleStatus, TimedStatus, UsageLedger,
};

/// How a tick's verdict is being carried, which decides what burns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    /// A window (or no schedule at all) decides. No pool burns.
    Window,
    /// A named time's own pool carries the device, as that time.
    NamedPool(String),
}

/// Everything one assessment reads. All borrowed: assessing never mutates.
pub struct TimesInputs<'a> {
    pub now: i64,
    pub schedule: Option<&'a GrantSchedule>,
    pub budget: Option<&'a GrantBudget>,
    /// `None` = no usable `times` clause (never defined, unparseable, or a `v`
    /// too high): every tagged window locks, untagged windows are unaffected.
    pub times: Option<&'a GrantTimes>,
    pub usage: &'a UsageLedger,
    pub extension: &'a ExtensionLedger,
    pub consolidated: Option<&'a ConsolidatedUsage>,
    pub stand_down: Option<StandDown>,
}

/// The one answer every reader shares.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// The time-left breakdown, exactly what `compute_remaining` would have
    /// said had it understood times. `schedule_secs` is the seconds to the end
    /// of the contiguous span of the same time (STATUS `windowLeftSecs`).
    pub rem: Remaining,
    /// The named time in force; `None` for Free time and when locked.
    pub active_time: Option<String>,
    /// How the device is carried. Only meaningful while unlocked.
    pub run: Run,
    /// The schedule's status under the times evaluator (`None` = no schedule).
    pub status: Option<TimedStatus>,
    /// Whether usage right now charges the budget and buckets. `true` for Free
    /// time, a counting named time, and whenever locked: the strict direction.
    pub counts_against_budget: bool,
    /// The schedule is closed this tick (outside every window, paused, or an
    /// unknown tag): Free's schedule pool is what burns, as today.
    pub burn_schedule_pool: bool,
}

/// Whether any window, weekly or dated, carries a `time` tag.
pub fn has_tagged_window(s: &GrantSchedule) -> bool {
    let tagged =
        |ws: &Vec<charter_schedule::GrantScheduleWindow>| ws.iter().any(|w| w.time.is_some());
    let w = &s.weekly;
    [&w.mon, &w.tue, &w.wed, &w.thu, &w.fri, &w.sat, &w.sun]
        .into_iter()
        .flatten()
        .any(tagged)
        || s.overrides.as_ref().is_some_and(|o| o.values().any(tagged))
}

/// The id predicate every times call takes: "does this device's `times`
/// clause define it?" Always false when there is no usable clause.
pub fn known_in(times: Option<&GrantTimes>) -> impl Fn(&str) -> bool + '_ {
    move |id| times.is_some_and(|t| t.get(id).is_some())
}

/// The schedule's status. A schedule with no tagged window is evaluated by the
/// classic evaluator (so legacy wards are untouched, including how adjacent
/// windows count down); one with tags goes through the times evaluator.
pub fn timed_status(schedule: &GrantSchedule, times: Option<&GrantTimes>, now: i64) -> TimedStatus {
    if has_tagged_window(schedule) {
        return evaluate_times(schedule, &known_in(times), now);
    }
    match evaluate_grant_schedule(schedule, now) {
        ScheduleStatus::Unbounded => TimedStatus::Unbounded,
        ScheduleStatus::Open { seconds_to_close } => TimedStatus::Open {
            time: None,
            span_end: now + seconds_to_close as i64,
        },
        ScheduleStatus::Locked { seconds_to_open } => TimedStatus::Locked {
            cause: if schedule.paused == Some(true) {
                LockCause::Paused
            } else {
                LockCause::OutsideWindows
            },
            seconds_to_open,
        },
        ScheduleStatus::Unparseable => TimedStatus::Unparseable,
    }
}

fn min_unbounded(a: i64, b: i64) -> i64 {
    match (a < 0, b < 0) {
        (true, true) => -1,
        (true, false) => b,
        (false, true) => a,
        (false, false) => a.min(b),
    }
}

/// Decide the tick. Pure.
pub fn assess(inp: &TimesInputs) -> Verdict {
    let now = inp.now;
    let known = known_in(inp.times);
    let status = inp.schedule.map(|s| timed_status(s, inp.times, now));

    // The time the window itself names, if it names one. A budget-exempt time
    // takes the BUDGET dimension out of the sum: "budget exhaustion never locks
    // a time that does not count". A window time the clause somehow does not
    // define cannot occur here (the evaluator locks those), but if it did, the
    // strict reading (it counts) is the default.
    let window_named: Option<&NamedTime> = match &status {
        Some(TimedStatus::Open { time: Some(id), .. }) => inp.times.and_then(|t| t.get(id)),
        _ => None,
    };
    let counts = window_named.is_none_or(|n| n.counts_against_budget());

    // Budget dimension: the quota arithmetic stays in core. Asking it with the
    // schedule withheld isolates the budget; the stand-down cap is applied
    // below, after both pools, exactly as `compute_remaining` orders it.
    let b = compute_remaining(&EnforcerInputs {
        now_unix: now,
        schedule: None,
        budget: if counts { inp.budget } else { None },
        usage: inp.usage,
        extension: inp.extension,
        consolidated: inp.consolidated,
        stand_down: None,
    });

    // Schedule dimension.
    let eod = (enforcement_eod_unix(inp.schedule, inp.budget, now) - now).max(0) as u64;
    let sched_ext = inp.extension.schedule_extra_secs(now);
    let sched_deduct = inp.extension.schedule_deducted_secs(now);
    let mut malformed = false;
    let mut next_open: Option<i64> = None;
    let schedule_secs: i64 = match &status {
        None | Some(TimedStatus::Unbounded) => -1,
        Some(TimedStatus::Unparseable) => {
            // Fail SAFE: an unreadable schedule locks whatever any pool says.
            malformed = true;
            0
        }
        Some(TimedStatus::Open { time, span_end }) => {
            let base = (span_end - now).max(0) as u64;
            // A Free window's own schedule pool carries on past its close, so
            // it is summed in (today's behaviour). A NAMED window leaves that
            // pool for after it ends: the posture changes at the span's end,
            // and the pool then carries the device as Free. Likewise a Free
            // span that a named window follows (it was carved): the pool is
            // not spent in between, so it is not part of this span's length.
            // An untagged schedule has no such seam and sums as it always did.
            let free_continues_into_closed = time.is_none()
                && inp.schedule.is_none_or(|s| {
                    !has_tagged_window(s)
                        || !matches!(
                            timed_status(s, inp.times, *span_end),
                            TimedStatus::Open { .. }
                        )
                });
            let ext = if free_continues_into_closed {
                sched_ext
            } else {
                0
            };
            let ded = if free_continues_into_closed {
                sched_deduct
            } else {
                0
            };
            (base + ext).saturating_sub(ded).min(eod) as i64
        }
        Some(TimedStatus::Locked {
            seconds_to_open, ..
        }) => {
            next_open = seconds_to_open.map(|s| s as i64);
            // Closed: only Free's schedule pool can carry it (then the named
            // pools below). Every cause is the same here, the unknown tag
            // included, so a granted ask is never "sent and nothing happened".
            sched_ext.saturating_sub(sched_deduct).min(eod) as i64
        }
    };
    let burn_schedule_pool = matches!(status, Some(TimedStatus::Locked { .. }));

    let charter_secs = min_unbounded(schedule_secs, b.budget_secs);
    let (effective, stood_down) = match inp.stand_down {
        Some(sd) => {
            let cap = sd.secs_until_lock.max(0);
            let capped = if charter_secs < 0 {
                cap
            } else {
                charter_secs.min(cap)
            };
            (capped, cap == 0)
        }
        None => (charter_secs, false),
    };
    let locked = effective == 0;
    let reason = if !locked {
        None
    } else if malformed {
        Some(LockReason::Malformed)
    } else if stood_down {
        Some(LockReason::StandDown)
    } else if schedule_secs == 0 {
        Some(LockReason::Schedule)
    } else {
        Some(LockReason::Budget)
    };
    let mut rem = Remaining {
        effective_secs: effective,
        schedule_secs,
        budget_secs: b.budget_secs,
        extension_secs: b.extension_secs,
        locked,
        reason,
        next_open_secs: if reason == Some(LockReason::Schedule) {
            next_open
        } else {
            None
        },
        budget_day_secs: b.budget_day_secs,
        budget_week_secs: b.budget_week_secs,
    };

    let mut run = Run::Window;
    let mut active_time = if rem.locked {
        None
    } else {
        window_named.map(|n| n.id.clone())
    };
    let mut counts_against_budget = counts;
    if rem.locked {
        // A locked tick is charged strictly (nothing accrues while frozen
        // anyway; this is the value the NEXT interval's credit reads).
        counts_against_budget = true;
    }

    // Step 2 of the fall-through: Free's pools did not carry it, so the named
    // pool most recently topped up (among times this device defines) runs the
    // device AS that time. Only a schedule or budget lock falls through: a
    // malformed clause and a stand-down never do, and the stand-down cap binds
    // the pool as it binds every other.
    let pool_time = if matches!(
        rem.reason,
        Some(LockReason::Schedule) | Some(LockReason::Budget)
    ) {
        inp.extension.most_recent_named_pool_among(now, &known)
    } else {
        None
    };
    if let Some(id) = &pool_time {
        let pool = inp.extension.time_extra_secs(now, id).min(eod) as i64;
        let eff = match inp.stand_down {
            Some(sd) => pool.min(sd.secs_until_lock.max(0)),
            None => pool,
        };
        if eff > 0 {
            rem.locked = false;
            rem.reason = None;
            rem.effective_secs = eff;
            // A pool-run span ends when the pool does.
            rem.schedule_secs = eff;
            rem.next_open_secs = None;
            run = Run::NamedPool(id.clone());
            active_time = Some(id.clone());
            counts_against_budget = inp
                .times
                .and_then(|t| t.get(id))
                .is_some_and(|n| n.counts_against_budget());
        }
    }

    Verdict {
        rem,
        active_time,
        run,
        status,
        counts_against_budget,
        burn_schedule_pool,
    }
}

// ---- the edge machine -------------------------------------------------------

/// The edge-triggered lock/warning state machine: `EnforcerCore::tick`'s own
/// logic, fed a [`Remaining`] this module computed instead of the one
/// `compute_remaining` would have. Core's `EnforcerCore` takes only inputs
/// (and so only the classic schedule), with no seam to hand it a verdict; this
/// is the same ~40 lines over a different source, and must move with it.
#[derive(Debug, Default)]
pub struct EdgeMachine {
    locked: bool,
    warned_ten: bool,
    warned_one: bool,
}

impl EdgeMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn tick(&mut self, rem: &Remaining) -> Vec<EnforcerEffect> {
        let mut effs = Vec::new();
        if rem.locked && !self.locked {
            self.locked = true;
            effs.push(EnforcerEffect::ShowLock(
                rem.reason.unwrap_or(LockReason::Schedule),
            ));
            effs.push(EnforcerEffect::Freeze(FreezeTarget::ManagedAppSlice));
            effs.push(EnforcerEffect::Audit(AuditKind::Locked));
        } else if !rem.locked && self.locked {
            self.locked = false;
            self.warned_ten = false;
            self.warned_one = false;
            effs.push(EnforcerEffect::Thaw(FreezeTarget::ManagedAppSlice));
            effs.push(EnforcerEffect::HideLock);
            effs.push(EnforcerEffect::Audit(AuditKind::Thawed));
        }
        if !self.locked {
            if rem.effective_secs < 0 || rem.effective_secs > 600 {
                self.warned_ten = false;
            }
            if rem.effective_secs < 0 || rem.effective_secs > 60 {
                self.warned_one = false;
            }
            let one_due = (0..=60).contains(&rem.effective_secs) && !self.warned_one;
            if (0..=600).contains(&rem.effective_secs) && !self.warned_ten {
                self.warned_ten = true;
                if !one_due {
                    effs.push(EnforcerEffect::Warn(WarnLevel::Ten));
                }
            }
            if one_due {
                self.warned_one = true;
                effs.push(EnforcerEffect::Warn(WarnLevel::One));
            }
        }
        effs.push(EnforcerEffect::TimeLeft(*rem));
        effs
    }
}

// ---- ward-facing copy -------------------------------------------------------

/// "Free time" for the background, else the clause's own label. An id this
/// device does not define reads as the bare id: never a blank.
pub fn label_of(times: Option<&GrantTimes>, id: Option<&str>) -> String {
    match id {
        None => "Free time".to_string(),
        Some(id) => times
            .and_then(|t| t.get(id))
            .map(|n| n.label.clone())
            .unwrap_or_else(|| id.to_string()),
    }
}

/// `HH:MM` of `unix` in `tz` (the schedule's own timezone; UTC if unreadable).
pub fn hhmm_at(tz: &str, unix: i64) -> String {
    use chrono::Timelike;
    let tz: chrono_tz::Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    match chrono::DateTime::from_timestamp(unix, 0) {
        Some(t) => {
            let l = t.with_timezone(&tz);
            format!("{:02}:{:02}", l.hour(), l.minute())
        }
        None => String::new(),
    }
}

/// The lock headline for a schedule lock on a schedule that uses times:
/// "Locked until Learning time at 09:00", "Locked until Free time tomorrow at
/// 07:00", "Locked today", or, for a window whose tag this device does not
/// define, "Nothing is set up for this time yet".
///
/// `None` means "use the classic copy" (the schedule uses no times at all).
pub fn locked_title(
    schedule: &GrantSchedule,
    times: Option<&GrantTimes>,
    status: &TimedStatus,
    now: i64,
) -> Option<String> {
    if !has_tagged_window(schedule) {
        return None;
    }
    let TimedStatus::Locked { cause, .. } = status else {
        return None;
    };
    if let LockCause::UnknownTime(_) = cause {
        return Some("Nothing is set up for this time yet".to_string());
    }
    Some(match next_window(schedule, &known_in(times), now) {
        Some(NextWindow {
            time,
            start_hhmm,
            day_offset,
            ..
        }) => {
            let label = label_of(times, time.as_deref());
            if day_offset == 0 {
                format!("Locked until {label} at {start_hhmm}")
            } else {
                format!("Locked until {label} tomorrow at {start_hhmm}")
            }
        }
        None => "Locked today".to_string(),
    })
}

/// What follows the current span, for the 10-/1-minute warnings: the next
/// span's label, or "locked". Evaluated AT the span's end, so a Free window
/// carved by a Learning one says "then Learning time".
pub fn then_label(
    schedule: &GrantSchedule,
    times: Option<&GrantTimes>,
    ext: &ExtensionLedger,
    span_end: i64,
) -> String {
    match timed_status(schedule, times, span_end) {
        TimedStatus::Open { time, .. } => label_of(times, time.as_deref()),
        TimedStatus::Unbounded => "Free time".to_string(),
        TimedStatus::Locked { .. } | TimedStatus::Unparseable => {
            // Closed at the end: a leftover Free schedule pool, then a named
            // pool, would carry on; otherwise it is locked.
            if ext.schedule_extra_secs(span_end) > 0 {
                "Free time".to_string()
            } else if let Some(id) = ext.most_recent_named_pool_among(span_end, known_in(times)) {
                label_of(times, Some(&id))
            } else {
                "locked".to_string()
            }
        }
    }
}

/// The labels of what an `onlyThese` time allows, for the home view: app
/// labels (from the device's inventory where known, else the package) then
/// site labels. Empty for `everythingExcept`.
pub fn allowed_labels(
    t: &NamedTime,
    label_for_pkg: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    if t.posture != TimePosture::OnlyThese {
        return Vec::new();
    }
    let mut out: Vec<String> = t
        .apps
        .iter()
        .map(|p| label_for_pkg(p).unwrap_or_else(|| p.clone()))
        .collect();
    out.extend(t.sites.iter().map(|s| s.label.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_schedule::clause::{GrantScheduleWindow, WeekStart, WeeklySchedule};
    use charter_schedule::Dimension;

    const TZ: &str = "Europe/London";
    // Mon 2026-06-29, Europe/London (BST = UTC+1).
    const MON_1700: i64 = 1_782_752_400 - 3600; // 17:00 BST
    const MON_1730: i64 = MON_1700 + 1800;
    const MON_1800: i64 = MON_1700 + 3600;
    const MON_2200: i64 = 1_782_766_800;
    const MON_1500: i64 = MON_1700 - 7200;

    fn win(s: &str, e: &str, t: Option<&str>) -> GrantScheduleWindow {
        GrantScheduleWindow {
            start: s.into(),
            end: e.into(),
            time: t.map(str::to_string),
        }
    }

    fn sched(mon: Vec<GrantScheduleWindow>) -> GrantSchedule {
        GrantSchedule {
            v: 1,
            tz: TZ.into(),
            paused: None,
            weekly: WeeklySchedule {
                mon: Some(mon),
                ..Default::default()
            },
            overrides: None,
            issued_at: 1,
        }
    }

    fn times_clause(counts: bool) -> GrantTimes {
        serde_json::from_value(serde_json::json!({
            "v": 1, "issuedAt": 1,
            "times": [
                {"id": "learning", "label": "Learning time", "posture": "onlyThese",
                 "apps": ["com.khan"], "countsAgainstBudget": counts},
                {"id": "reading", "label": "Reading", "posture": "everythingExcept"}
            ]
        }))
        .unwrap()
    }

    fn budget(daily: u32) -> GrantBudget {
        serde_json::from_value(serde_json::json!({
            "v": 1, "tz": TZ, "dailyMinutes": daily, "weekStart": "mon", "issuedAt": 1
        }))
        .unwrap()
    }

    struct Fixture {
        usage: UsageLedger,
        ext: ExtensionLedger,
    }
    fn fx(now: i64) -> Fixture {
        Fixture {
            usage: UsageLedger::new(TZ, WeekStart::Mon, now),
            ext: ExtensionLedger::new(TZ, now),
        }
    }

    fn run<'a>(
        f: &'a Fixture,
        now: i64,
        s: Option<&'a GrantSchedule>,
        b: Option<&'a GrantBudget>,
        t: Option<&'a GrantTimes>,
    ) -> Verdict {
        assess(&TimesInputs {
            now,
            schedule: s,
            budget: b,
            times: t,
            usage: &f.usage,
            extension: &f.ext,
            consolidated: None,
            stand_down: None,
        })
    }

    #[test]
    fn an_untagged_schedule_assesses_exactly_as_compute_remaining() {
        // The regression pin: a ward whose guardian never uses times must
        // compute what it always computed, across open, closed, extended and
        // budget-spent states.
        let s = sched(vec![win("16:00", "20:00", None)]);
        let b = budget(60);
        for now in [MON_1500, MON_1700, MON_1800, MON_2200] {
            for (sched_min, budget_min, spent) in
                [(0u16, 0u16, 0u64), (30, 0, 0), (0, 30, 0), (15, 15, 4000)]
            {
                let mut f = fx(now);
                if sched_min > 0 {
                    f.ext.apply(now, "s", sched_min, Dimension::Schedule);
                }
                if budget_min > 0 {
                    f.ext.apply(now, "b", budget_min, Dimension::Budget);
                }
                if spent > 0 {
                    f.usage
                        .credit(now, charter_schedule::Activity::Active, spent);
                }
                let classic = compute_remaining(&EnforcerInputs {
                    now_unix: now,
                    schedule: Some(&s),
                    budget: Some(&b),
                    usage: &f.usage,
                    extension: &f.ext,
                    consolidated: None,
                    stand_down: None,
                });
                let v = run(&f, now, Some(&s), Some(&b), None);
                assert_eq!(
                    v.rem, classic,
                    "now={now} sched={sched_min} budget={budget_min}"
                );
                assert_eq!(v.active_time, None);
            }
        }
        // Budget-only and schedule-only wards too.
        let f = fx(MON_1800);
        let only_b = compute_remaining(&EnforcerInputs {
            now_unix: MON_1800,
            schedule: None,
            budget: Some(&b),
            usage: &f.usage,
            extension: &f.ext,
            consolidated: None,
            stand_down: None,
        });
        assert_eq!(run(&f, MON_1800, None, Some(&b), None).rem, only_b);
    }

    #[test]
    fn a_free_span_followed_by_a_named_one_does_not_count_frees_pool_in_its_length() {
        let s = sched(vec![
            win("16:00", "19:00", None),
            win("17:00", "18:00", Some("learning")),
        ]);
        let t = times_clause(false);
        let mut f = fx(MON_1700 - 1800);
        f.ext.apply(MON_1700 - 1800, "s", 30, Dimension::Schedule);
        let v = run(&f, MON_1700 - 600, Some(&s), None, Some(&t)); // 16:50
        assert_eq!(v.rem.schedule_secs, 600, "Free's span ends at 17:00");
        // After the carve the Free span runs to a CLOSE, where the pool carries on.
        let v = run(&f, MON_1800 + 600, Some(&s), None, Some(&t)); // 18:10
        assert_eq!(v.rem.schedule_secs, 3000 + 1800);
    }

    #[test]
    fn a_tagged_window_runs_its_time_and_the_window_decides() {
        let s = sched(vec![
            win("16:00", "20:00", None),
            win("17:00", "18:00", Some("learning")),
        ]);
        let t = times_clause(false);
        let f = fx(MON_1730);
        let v = run(&f, MON_1730, Some(&s), None, Some(&t));
        assert!(!v.rem.locked);
        assert_eq!(v.active_time.as_deref(), Some("learning"));
        assert_eq!(v.run, Run::Window);
        // windowLeftSecs is the span end (18:00), not the carved Free window's.
        assert_eq!(v.rem.schedule_secs, 1800);
    }

    #[test]
    fn an_unknown_tag_locks_and_never_runs_as_free() {
        let s = sched(vec![
            win("16:00", "20:00", None),
            win("17:00", "18:00", Some("ghost")),
        ]);
        let t = times_clause(false);
        let f = fx(MON_1730);
        // The classic evaluator calls this Open: that is the fail-open the
        // override exists to close.
        assert!(matches!(
            evaluate_grant_schedule(&s, MON_1730),
            ScheduleStatus::Open { .. }
        ));
        let v = run(&f, MON_1730, Some(&s), None, Some(&t));
        assert!(v.rem.locked);
        assert_eq!(v.rem.reason, Some(LockReason::Schedule));
        assert!(matches!(
            v.status,
            Some(TimedStatus::Locked { cause: LockCause::UnknownTime(ref id), .. }) if id == "ghost"
        ));
        // No times clause at all: every tagged window locks.
        let v = run(&f, MON_1730, Some(&s), None, None);
        assert!(v.rem.locked, "no times clause: tagged span is locked");
        // The untagged hour either side is untouched.
        let v = run(&f, MON_1700 - 600, Some(&s), None, None);
        assert!(!v.rem.locked);
    }

    #[test]
    fn an_unknown_tag_span_is_carried_by_frees_schedule_pool_and_burns_it() {
        let s = sched(vec![win("17:00", "18:00", Some("ghost"))]);
        let t = times_clause(false);
        let mut f = fx(MON_1730);
        f.ext.apply(MON_1730, "free-30", 30, Dimension::Schedule);
        let v = run(&f, MON_1730, Some(&s), None, Some(&t));
        assert!(!v.rem.locked, "Free's pool carries a closed schedule");
        assert_eq!(v.active_time, None, "as Free, not as the unknown tag");
        assert!(v.burn_schedule_pool);
    }

    #[test]
    fn a_non_counting_time_ignores_a_spent_budget_but_free_does_not() {
        let s = sched(vec![
            win("16:00", "20:00", None),
            win("17:00", "18:00", Some("learning")),
        ]);
        let b = budget(60);
        let t = times_clause(false);
        let mut f = fx(MON_1730);
        f.usage
            .credit(MON_1730, charter_schedule::Activity::Active, 3600);
        // Inside Learning: budget spent, yet it runs.
        let v = run(&f, MON_1730, Some(&s), Some(&b), Some(&t));
        assert!(!v.rem.locked);
        assert_eq!(v.active_time.as_deref(), Some("learning"));
        assert!(!v.counts_against_budget);
        // Inside the Free part of the same window: budget spent, locked.
        let v = run(&f, MON_1700 - 600, Some(&s), Some(&b), Some(&t));
        assert!(v.rem.locked);
        assert_eq!(v.rem.reason, Some(LockReason::Budget));
        // A counting named time does NOT escape the budget.
        let t = times_clause(true);
        let v = run(&f, MON_1730, Some(&s), Some(&b), Some(&t));
        assert!(
            v.rem.locked,
            "countsAgainstBudget: true is bound by the budget"
        );
    }

    #[test]
    fn a_named_pool_unlocks_a_budget_spent_child_as_that_time() {
        // The design's worked example: 30 minutes of Learning time at 17:30 to
        // a child whose Free budget ran out inside a Free window.
        let s = sched(vec![win("16:00", "19:00", None)]);
        let b = budget(60);
        let t = times_clause(false);
        let mut f = fx(MON_1730);
        f.usage
            .credit(MON_1730, charter_schedule::Activity::Active, 3600);
        assert!(run(&f, MON_1730, Some(&s), Some(&b), Some(&t)).rem.locked);
        assert!(f.ext.apply_time(MON_1730, "g1", 30, "learning"));
        let v = run(&f, MON_1730, Some(&s), Some(&b), Some(&t));
        assert!(!v.rem.locked);
        assert_eq!(v.active_time.as_deref(), Some("learning"));
        assert_eq!(v.run, Run::NamedPool("learning".into()));
        assert_eq!(v.rem.effective_secs, 1800);
        assert_eq!(
            v.rem.schedule_secs, 1800,
            "a pool-run span ends when the pool does"
        );
        // Burning it counts it down, and at zero the device locks again.
        f.ext.burn_time(MON_1730 + 1800, "learning", 1800);
        let v = run(&f, MON_1730 + 1800, Some(&s), Some(&b), Some(&t));
        assert!(v.rem.locked);
    }

    #[test]
    fn frees_pool_is_tried_before_a_named_pool() {
        let s = sched(vec![win("16:00", "17:00", None)]);
        let t = times_clause(false);
        let mut f = fx(MON_1800);
        f.ext.apply_time(MON_1800, "t1", 30, "learning");
        f.ext.apply(MON_1800, "s1", 10, Dimension::Schedule);
        let v = run(&f, MON_1800, Some(&s), None, Some(&t));
        assert!(!v.rem.locked);
        assert_eq!(v.active_time, None, "Free's 10 minutes first, as Free");
        assert_eq!(v.rem.effective_secs, 600);
        assert_eq!(v.run, Run::Window);
        assert!(v.burn_schedule_pool);
    }

    #[test]
    fn a_pool_for_a_time_the_device_does_not_define_carries_nothing() {
        let s = sched(vec![win("16:00", "17:00", None)]);
        let t = times_clause(false);
        let mut f = fx(MON_1800);
        f.ext.apply_time(MON_1800, "t1", 30, "ghost");
        let v = run(&f, MON_1800, Some(&s), None, Some(&t));
        assert!(v.rem.locked, "an unknown id enacts no extra time");
        // ...but it is kept: the clause arriving later the same day honours it.
        let later = GrantTimes {
            v: 1,
            issued_at: 2,
            times: {
                let mut x = times_clause(false).times;
                x.push(
                    serde_json::from_value(serde_json::json!({
                        "id": "ghost", "label": "Ghost", "posture": "everythingExcept"
                    }))
                    .unwrap(),
                );
                x
            },
        };
        let v = run(&f, MON_1800, Some(&s), None, Some(&later));
        assert!(!v.rem.locked);
        assert_eq!(v.active_time.as_deref(), Some("ghost"));
    }

    #[test]
    fn the_most_recently_topped_named_pool_wins() {
        let s = sched(vec![win("16:00", "17:00", None)]);
        let t = times_clause(false);
        let mut f = fx(MON_1800);
        f.ext.apply_time(MON_1800, "a", 30, "learning");
        f.ext.apply_time(MON_1800, "b", 20, "reading");
        let v = run(&f, MON_1800, Some(&s), None, Some(&t));
        assert_eq!(v.active_time.as_deref(), Some("reading"));
        assert_eq!(v.rem.effective_secs, 1200);
    }

    #[test]
    fn a_stand_down_and_a_malformed_schedule_are_never_carried_by_a_pool() {
        let s = sched(vec![win("16:00", "17:00", None)]);
        let t = times_clause(false);
        let mut f = fx(MON_1800);
        f.ext.apply_time(MON_1800, "a", 30, "learning");
        let v = assess(&TimesInputs {
            now: MON_1800,
            schedule: Some(&s),
            budget: None,
            times: Some(&t),
            usage: &f.usage,
            extension: &f.ext,
            consolidated: None,
            stand_down: Some(StandDown { secs_until_lock: 0 }),
        });
        assert!(v.rem.locked);
        assert_eq!(v.rem.reason, Some(LockReason::StandDown));
        let mut bad = s.clone();
        bad.tz = "Not/AZone".into();
        let v = run(&f, MON_1800, Some(&bad), None, Some(&t));
        assert!(v.rem.locked);
        assert_eq!(v.rem.reason, Some(LockReason::Malformed));
    }

    #[test]
    fn the_stand_down_cap_binds_a_pool_run() {
        let s = sched(vec![win("16:00", "17:00", None)]);
        let t = times_clause(false);
        let mut f = fx(MON_1800);
        f.ext.apply_time(MON_1800, "a", 30, "learning");
        let v = assess(&TimesInputs {
            now: MON_1800,
            schedule: Some(&s),
            budget: None,
            times: Some(&t),
            usage: &f.usage,
            extension: &f.ext,
            consolidated: None,
            stand_down: Some(StandDown {
                secs_until_lock: 90,
            }),
        });
        assert!(!v.rem.locked);
        assert_eq!(v.rem.effective_secs, 90);
    }

    #[test]
    fn lock_copy_names_what_comes_next() {
        let s = sched(vec![
            win("09:00", "12:00", Some("learning")),
            win("16:00", "19:00", None),
        ]);
        let t = times_clause(false);
        let f = fx(MON_1500);
        let early = MON_1500 - 7 * 3600; // 08:00
        let st = run(&f, early, Some(&s), None, Some(&t)).status.unwrap();
        assert_eq!(
            locked_title(&s, Some(&t), &st, early).as_deref(),
            Some("Locked until Learning time at 09:00")
        );
        // Between the two: next up is Free time at 16:00.
        let now = MON_1500 - 3600;
        let st = run(&f, now, Some(&s), None, Some(&t)).status.unwrap();
        assert_eq!(
            locked_title(&s, Some(&t), &st, now).as_deref(),
            Some("Locked until Free time at 16:00")
        );
        // After the last window: nothing today or tomorrow's first.
        let st = run(&f, MON_2200, Some(&s), None, Some(&t)).status.unwrap();
        let title = locked_title(&s, Some(&t), &st, MON_2200).unwrap();
        assert!(
            title == "Locked today" || title.starts_with("Locked until"),
            "{title}"
        );
        // Unknown tag.
        let s2 = sched(vec![win("16:00", "19:00", Some("ghost"))]);
        let st = run(&f, MON_1730, Some(&s2), None, Some(&t)).status.unwrap();
        assert_eq!(
            locked_title(&s2, Some(&t), &st, MON_1730).as_deref(),
            Some("Nothing is set up for this time yet")
        );
        // A legacy schedule keeps the classic copy.
        let legacy = sched(vec![win("16:00", "19:00", None)]);
        let st = run(&f, MON_2200, Some(&legacy), None, None).status.unwrap();
        assert_eq!(locked_title(&legacy, None, &st, MON_2200), None);
    }

    #[test]
    fn then_label_looks_across_the_span_end() {
        let s = sched(vec![
            win("16:00", "19:00", None),
            win("17:00", "18:00", Some("learning")),
        ]);
        let t = times_clause(false);
        let f = fx(MON_1700);
        assert_eq!(then_label(&s, Some(&t), &f.ext, MON_1800), "Free time");
        assert_eq!(then_label(&s, Some(&t), &f.ext, MON_1700), "Learning time");
        assert_eq!(then_label(&s, Some(&t), &f.ext, MON_2200), "locked");
    }

    #[test]
    fn the_edge_machine_matches_the_core_enforcer() {
        // Drive both over the same untagged inputs and compare effects: the
        // copy of EnforcerCore::tick must not drift from the original.
        let s = sched(vec![win("16:00", "17:00", None)]);
        let mut core = charter_schedule::EnforcerCore::new();
        let mut mine = EdgeMachine::new();
        let f = fx(MON_1700 - 1200);
        for now in (MON_1700 - 1200..MON_1700 + 600).step_by(30) {
            let inp = EnforcerInputs {
                now_unix: now,
                schedule: Some(&s),
                budget: None,
                usage: &f.usage,
                extension: &f.ext,
                consolidated: None,
                stand_down: None,
            };
            let a = core.tick(&inp);
            let v = run(&f, now, Some(&s), None, None);
            let b = mine.tick(&v.rem);
            assert_eq!(a, b, "diverged at {now}");
        }
    }
}
