//! Times (§Times) on Linux — the routing half, pure and unit-tested.
//!
//! The shared enforcer core (`MultiChildEnforcer` → `compute_remaining`) still
//! evaluates the schedule with the classic evaluator, which runs a TAGGED
//! window as Free time: that is exactly what a pre-times ward does, and the
//! core is shared with Android, so it is left alone. Every decision that
//! needs to know WHICH time the device is in is made here, between the core's
//! verdict and the things that act on it (the freeze, the lock card, the
//! per-app sweep, STATUS, the ward's own readout).
//!
//! # Fail direction
//!
//! The times layer can do two things to the core's verdict, and they fail in
//! opposite directions on purpose:
//!
//! - It may LOCK an instant the core would open: a tagged window whose id the
//!   `times` clause does not define (or no usable clause at all), and an
//!   *only these* time while the app inventory is unknown. Both are
//!   restrictions the core cannot see; opening them as Free time would grant
//!   exactly what the guardian meant to withhold.
//! - It may OPEN an instant the core would lock, but ONLY when the core's
//!   lock is a dimension a named time is entitled to ignore: the budget, for
//!   a named window that does not count against it; or the schedule/budget
//!   dimension, for a named pool the guardian topped up. A stand-down or a
//!   malformed clause outranks every time — nothing here lifts those.
//!
//! Everything else keeps the core's verdict, byte for byte. A family with no
//! `times` clause and no tagged window never reaches a branch that differs
//! from the pre-times daemon.

use charter_ipc::dto::{TimeChoice, TimesView, FREE_TIME_CHOICE};
use charter_proto::status::AppRef;
use charter_proto::{LearningApp, LearningAppKind};
use charter_schedule::{
    Activity, Dimension, EnforcerEffect, GrantSchedule, LockCause, LockReason, Remaining,
    TimedStatus, WarnLevel,
};
use charter_spine::multi_child::{ChildDecision, PendingExtension};
use charter_spine::times::{GrantTimes, NamedTime, TimePosture};
use chrono::{TimeZone, Timelike};

/// What Free time is called on every ward-facing surface.
pub const FREE_TIME_LABEL: &str = "Free time";

/// The ward-facing copy for a span whose tag this device does not know.
pub const UNKNOWN_TIME_DETAIL: &str = "Nothing is set up for this time yet.";

/// The `times` clause in force, or `None` — "absent". An unparseable body, or
/// one whose `v` is too high, is treated as absent (`parse_or_absent`): every
/// tagged window then LOCKS and untagged windows are unaffected.
pub fn parse_times(json: Option<&str>) -> Option<GrantTimes> {
    json.and_then(GrantTimes::parse_or_absent)
}

/// Whether any window in the schedule (weekly or any override) carries a time
/// tag. Governs who owns the 10-/1-minute warnings: with no tag anywhere the
/// core's Free-time warnings are exactly right and are kept.
pub fn schedule_has_tags(s: &GrantSchedule) -> bool {
    let w = &s.weekly;
    [&w.mon, &w.tue, &w.wed, &w.thu, &w.fri, &w.sat, &w.sun]
        .into_iter()
        .flat_map(|d| d.iter().flatten())
        .chain(s.overrides.iter().flat_map(|o| o.values().flatten()))
        .any(|win| win.time.is_some())
}

/// Why the times layer itself locks an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimesLock {
    /// A tagged window whose id the `times` clause does not define, or no
    /// usable `times` clause at all. Carries the tag.
    UnknownTime(String),
    /// An *only these* time while the app inventory has never answered. Its
    /// allowlist is scoped to the inventory, and "an empty inventory blocks
    /// nothing" would run the time as Free — a state a ward can arrange with
    /// no privilege (a hung FUSE mount under their own launcher dir).
    /// Carries the time's id.
    InventoryUnknown(String),
}

/// Which time the device is in for one child, as far as enforcement goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Free time — or a lock nothing in the times layer may lift. The core's
    /// own verdict stands. `span_end` is the end of the contiguous Free span
    /// when it is cut short by a different time (so the readout can say so);
    /// `None` when the core's own figure is already the truth.
    Core { span_end: Option<i64> },
    /// Running a named time: unlocked, whatever Free's budget says.
    Named {
        id: String,
        /// Running off the time's own extension pool (out of any window that
        /// can run) rather than a window. The pool burns wall clock while
        /// this is true, and only then.
        from_pool: bool,
        /// `countsAgainstBudget`: whether usage in it charges the budget.
        counts: bool,
        /// When the contiguous span of this time (or its pool) ends — what
        /// `windowLeftSecs` counts down to, per the contract.
        span_end: i64,
        /// When this time actually ends for the ward (unix seconds): the
        /// span end, or sooner when a counting time's budget (or a
        /// stand-down's grace) runs out first.
        ends_at: i64,
        /// Whether `ends_at` is the end of the span/pool (so something else
        /// may follow) rather than the budget running out (then: locked).
        span_limited: bool,
    },
    /// Locked by the times layer itself, whatever the core says.
    Locked(TimesLock),
}

/// What [`decide`] needs. `core` is Free time's verdict from the shared core
/// (schedule and budget pools already folded in — fall-through step 1);
/// `pool` is the named pool most recently topped up that still has minutes,
/// restricted to ids the `times` clause defines (fall-through step 2).
pub struct Inputs<'a> {
    pub timed: &'a TimedStatus,
    pub core: &'a Remaining,
    pub times: Option<&'a GrantTimes>,
    pub inventory_known: bool,
    pub pool: Option<(&'a str, u64)>,
    pub now: i64,
}

/// A named time, run — unless it is *only these* and the inventory is
/// unknown, which locks (see [`TimesLock::InventoryUnknown`]).
fn run_named(i: &Inputs, t: &NamedTime, from_pool: bool, span_end: i64, ends_at: i64) -> Verdict {
    if t.posture == TimePosture::OnlyThese && !i.inventory_known {
        return Verdict::Locked(TimesLock::InventoryUnknown(t.id.clone()));
    }
    Verdict::Named {
        id: t.id.clone(),
        from_pool,
        counts: t.counts_against_budget(),
        span_end,
        ends_at,
        span_limited: ends_at >= span_end,
    }
}

/// The fall-through when the window's own time cannot run (§Times, in the
/// spec's order). Step 1 — Free's pool for the locking dimension — already
/// happened inside the core: if it had minutes the core would not be locked.
/// So here: only when the core's lock is one of `dims` (never a stand-down
/// or a malformed clause), the most recently topped-up named pool runs AS
/// that named time; nothing left ⇒ the core's lock stands.
fn fall_through(i: &Inputs, dims: &[LockReason]) -> Verdict {
    let dimension_lock = i.core.locked && i.core.reason.is_some_and(|r| dims.contains(&r));
    if !dimension_lock {
        return Verdict::Core { span_end: None };
    }
    match i.pool {
        Some((id, secs)) if secs > 0 => match i.times.and_then(|ts| ts.get(id)) {
            Some(t) => run_named(i, t, true, i.now + secs as i64, i.now + secs as i64),
            // `pool` is filtered to known ids, so this is unreachable — and
            // if it ever is reached, a pool for a time nobody defined enacts
            // nothing (the contract's "unknown id ⇒ no extra time").
            None => Verdict::Core { span_end: None },
        },
        _ => Verdict::Core { span_end: None },
    }
}

/// Which time is it, and may it run? See the module docs for the fail
/// direction of every arm.
pub fn decide(i: &Inputs) -> Verdict {
    let core = i.core;
    match i.timed {
        // Hard lock, no pool fall-through: the span is locked "for that span"
        // (fail-safe table). The core runs this window as Free, so its
        // schedule pool is not burning here either — letting any pool open it
        // would be a pool that never runs down.
        TimedStatus::Locked {
            cause: LockCause::UnknownTime(tag),
            ..
        } => Verdict::Locked(TimesLock::UnknownTime(tag.clone())),
        TimedStatus::Open {
            time: Some(id),
            span_end,
        } => {
            // `evaluate_times` only opens a known tag; re-checked rather than
            // trusted, and an absent definition LOCKS (never runs as Free).
            let Some(t) = i.times.and_then(|ts| ts.get(id)) else {
                return Verdict::Locked(TimesLock::UnknownTime(id.clone()));
            };
            let counts = t.counts_against_budget();
            if !core.locked {
                // A counting time also ends when the budget does.
                let budget_end =
                    (counts && core.budget_secs >= 0).then(|| i.now + core.budget_secs);
                return match budget_end {
                    Some(b) if b < *span_end => run_named(i, t, false, *span_end, b),
                    _ => run_named(i, t, false, *span_end, *span_end),
                };
            }
            match core.reason {
                // `countsAgainstBudget: false`: budget exhaustion never locks
                // this time — the window decides and no pool burns.
                Some(LockReason::Budget) if !counts => run_named(i, t, false, *span_end, *span_end),
                // A counting time whose budget is spent cannot run: fall
                // through exactly as Free time would.
                Some(LockReason::Budget) => fall_through(i, &[LockReason::Budget]),
                // A stand-down, a malformed clause, or a schedule lock inside
                // an open window (only a guardian/admin take-back can produce
                // that) are deliberate and stand.
                _ => Verdict::Core { span_end: None },
            }
        }
        TimedStatus::Open {
            time: None,
            span_end,
        } => {
            if core.locked {
                fall_through(i, &[LockReason::Budget])
            } else {
                Verdict::Core {
                    span_end: Some(*span_end),
                }
            }
        }
        TimedStatus::Unbounded => {
            if core.locked {
                fall_through(i, &[LockReason::Budget])
            } else {
                Verdict::Core { span_end: None }
            }
        }
        // Outside every window (or a dormant schedule). Unlocked here means
        // Free's own schedule pool is running (step 1) — Free first.
        TimedStatus::Locked { .. } => {
            if core.locked {
                fall_through(i, &[LockReason::Schedule, LockReason::Budget])
            } else {
                Verdict::Core { span_end: None }
            }
        }
        // The core fails this SAFE (a `Malformed` lock); nothing to add.
        TimedStatus::Unparseable => Verdict::Core { span_end: None },
    }
}

/// One child's times state for one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimesState {
    pub timed: TimedStatus,
    pub verdict: Verdict,
    /// The core's [`Remaining`] with the verdict folded in — what every
    /// surface (freeze, lock card, STATUS, D-Bus readout) reads.
    pub remaining: Remaining,
    /// Whether the times layer owns this child's warnings this tick: the
    /// schedule has a tagged window, or a named time is running. When false
    /// the core's Free-time warnings are exactly right and are kept.
    pub in_play: bool,
    /// The instant the running time ends because its SPAN (or pool) ends —
    /// so something may follow. `None` when it ends because the budget does,
    /// or does not end today.
    pub span_end: Option<i64>,
}

/// Evaluate the times layer for one child.
pub fn resolve(
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    core: &Remaining,
    inventory_known: bool,
    pool: Option<(&str, u64)>,
    now: i64,
) -> TimesState {
    let timed = match schedule {
        Some(s) => charter_spine::times::evaluate_times(s, times, now),
        // No schedule clause: no window can carry a tag. Free, unbounded.
        None => TimedStatus::Unbounded,
    };
    let mut verdict = decide(&Inputs {
        timed: &timed,
        core,
        times,
        inventory_known,
        pool,
        now,
    });
    // A Free span is only "cut short" when a DIFFERENT time (or an unknown
    // tag) follows it. When the span runs out into "outside every window",
    // the core's own figure is the truth — it already includes Free's
    // schedule pool, which takes over at the close.
    if let (Verdict::Core { span_end: Some(e) }, Some(s)) = (&verdict, schedule) {
        let follows = charter_spine::times::evaluate_times(s, times, *e);
        if matches!(
            follows,
            TimedStatus::Locked {
                cause: LockCause::OutsideWindows | LockCause::Paused,
                ..
            }
        ) {
            verdict = Verdict::Core { span_end: None };
        }
    }
    let remaining = adjusted_remaining(core, &verdict, &timed, now);
    let span_end = match &verdict {
        Verdict::Named {
            ends_at,
            span_limited: true,
            ..
        } => Some(*ends_at),
        Verdict::Core { span_end: Some(e) }
            if !remaining.locked && remaining.effective_secs == (e - now).max(0) =>
        {
            Some(*e)
        }
        _ => None,
    };
    let in_play =
        schedule.is_some_and(schedule_has_tags) || !matches!(verdict, Verdict::Core { .. });
    TimesState {
        timed,
        verdict,
        remaining,
        in_play,
        span_end,
    }
}

/// The core's breakdown with the verdict folded in.
///
/// - **Named**: unlocked; the window/pool left is both the window and the
///   effective figure (`windowLeftSecs` = seconds to the end of the
///   contiguous span of that time, or of the pool). The budget figures are
///   left as they are — they describe Free time's day, which is still true.
/// - **Locked**: locked by schedule (`lockReason: 'schedule'` on the wire so a
///   current guardian app parses it), with the next runnable window.
/// - **Core**: unchanged, except that a Free span cut short by another time
///   reads as the span, and a schedule lock reports the next window that can
///   actually RUN (the classic evaluator would name an unknown-tag window).
pub fn adjusted_remaining(
    core: &Remaining,
    v: &Verdict,
    timed: &TimedStatus,
    now: i64,
) -> Remaining {
    let mut r = *core;
    match v {
        Verdict::Named {
            span_end, ends_at, ..
        } => {
            r.locked = false;
            r.reason = None;
            r.next_open_secs = None;
            r.effective_secs = (ends_at - now).max(0);
            r.schedule_secs = (span_end - now).max(0);
        }
        Verdict::Locked(_) => {
            r.locked = true;
            r.reason = Some(LockReason::Schedule);
            r.effective_secs = 0;
            r.schedule_secs = 0;
            r.next_open_secs = match timed {
                TimedStatus::Locked {
                    seconds_to_open, ..
                } => seconds_to_open.map(|s| s as i64),
                _ => None,
            };
        }
        Verdict::Core { span_end } => {
            if !core.locked {
                if let Some(e) = span_end {
                    let span = (e - now).max(0);
                    let clamp = |x: i64| if x < 0 { span } else { x.min(span) };
                    r.schedule_secs = clamp(core.schedule_secs);
                    r.effective_secs = clamp(core.effective_secs);
                }
            } else if core.reason == Some(LockReason::Schedule) {
                if let TimedStatus::Locked {
                    seconds_to_open, ..
                } = timed
                {
                    r.next_open_secs = seconds_to_open.map(|s| s as i64);
                }
            }
        }
    }
    r
}

/// [`resolve`] for one tracked child, reading Free time's verdict and the
/// named pools from the live enforcer. `None` when the uid is not tracked.
/// The pool offered is the most recently topped-up one among the ids the
/// child's `times` clause defines — an unknown id's minutes are skipped, not
/// discarded, so a clause that lands later today still honours them.
///
/// `stand_down_secs` is a guardian stand-down's countdown when one stands
/// (see [`apply_stand_down`]).
pub fn state_for(
    multi: &charter_spine::multi_child::MultiChildEnforcer,
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    inventory_known: bool,
    stand_down_secs: Option<i64>,
    uid: u32,
    now: i64,
) -> Option<TimesState> {
    let core = multi.remaining(uid, now)?;
    let pool_id = multi.most_recent_time_pool(uid, now, |id| is_known(times, id));
    let pool = pool_id
        .as_deref()
        .map(|id| (id, multi.time_extra(uid, id, now).unwrap_or(0)));
    let mut st = resolve(schedule, times, &core, inventory_known, pool, now);
    apply_stand_down(&mut st, stand_down_secs, now);
    Some(st)
}

/// A stand-down's grace ("finish up now") caps a running named time too. The
/// core caps Free time itself and, once the grace is spent, locks with
/// `StandDown` — which no named time lifts ([`decide`]). In the grace minutes
/// before that the named time's own figure would otherwise read as its whole
/// span, and the ward would get no "then locked" warning for a lock that is
/// about to land.
pub fn apply_stand_down(st: &mut TimesState, stand_down_secs: Option<i64>, now: i64) {
    let Some(sd) = stand_down_secs else {
        return;
    };
    if let Verdict::Named {
        ends_at,
        span_limited,
        ..
    } = &mut st.verdict
    {
        let cap = now + sd.max(0);
        if cap < *ends_at {
            *ends_at = cap;
            *span_limited = false;
            st.span_end = None;
            // `schedule_secs` stays the span (windowLeftSecs); only the
            // figure the ward actually has left is capped.
            st.remaining.effective_secs = sd.max(0);
        }
    }
}

/// Fold the verdict into the core's per-child decision. The freeze and the
/// lock screen are level-triggered on `locked` (see `apply_child_decisions`),
/// so overriding the level is all it takes; the core's edge effects are not
/// re-read for either.
///
/// While the times layer owns this child's warnings ([`TimesState::in_play`])
/// the core's `Warn` effects are dropped: they count down to the end of
/// Free's WINDOW, which inside a carved span is not when the ward's time ends.
pub fn apply_to_decision(d: &mut ChildDecision, s: &TimesState) {
    match &s.verdict {
        Verdict::Named { .. } => {
            d.locked = false;
            d.reason = None;
        }
        Verdict::Locked(_) => {
            d.locked = true;
            d.reason = Some(LockReason::Schedule);
        }
        Verdict::Core { .. } => {}
    }
    if s.in_play {
        d.effects.retain(|e| !matches!(e, EnforcerEffect::Warn(_)));
    }
}

/// The `Activity` the core charges the active child at. The core's only lever
/// for "charge nothing" is an activity that does not count: it gates the
/// screen/learning credit, every named-bucket credit and the unrecognised
/// meter on `activity.counts()`. A named time with `countsAgainstBudget:
/// false` therefore charges as `Idle` — usage in it drains neither the budget
/// nor any bucket. Every other state charges exactly as before.
pub fn charge_activity(base: Activity, v: &Verdict) -> Activity {
    match v {
        Verdict::Named { counts: false, .. } if base.counts() => Activity::Idle,
        _ => base,
    }
}

/// The named pool a running pool-run burns this tick, if any (wall clock,
/// every tick it runs — never usage, so an idle ward cannot stretch it).
pub fn burning_pool(v: &Verdict) -> Option<&str> {
    match v {
        Verdict::Named {
            id,
            from_pool: true,
            ..
        } => Some(id),
        _ => None,
    }
}

/// The running named time's definition, if any.
pub fn active_time<'a>(v: &Verdict, times: Option<&'a GrantTimes>) -> Option<&'a NamedTime> {
    match v {
        Verdict::Named { id, .. } => times?.get(id),
        _ => None,
    }
}

/// Where a guardian-approved `time.extend` waiting in the inbox lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingRoute<'a> {
    /// A named bucket's own pool (`limitHit: bucket`).
    Bucket(&'a str),
    /// A named TIME's own pool (§Times). Never Free's whole-device pools:
    /// landing "30 minutes of Learning time" there would unlock as Free.
    Time(&'a str),
    /// Free's whole-device schedule/budget pool.
    Dimension(Dimension),
    /// Nothing routable (should not occur — the enactor always sets one).
    Nothing,
}

/// The drain's routing rule. The bucket arm first (a `time` riding on a
/// bucket-hit grant is ignored — bucket routing wins), then the typed arm,
/// then Free's dimension. A drain that predates times matched `(None, None)`
/// and dropped a typed grant on the floor.
pub fn pending_route(p: &PendingExtension) -> PendingRoute<'_> {
    match (&p.bucket_id, &p.time_id, p.dim) {
        (Some(b), _, _) => PendingRoute::Bucket(b),
        (None, Some(t), _) => PendingRoute::Time(t),
        (None, None, Some(dim)) => PendingRoute::Dimension(dim),
        (None, None, None) => PendingRoute::Nothing,
    }
}

/// Whether the `times` clause in force defines `id`. An id it does not define
/// enacts no extra time: its minutes stay in the ledger (a clause that lands
/// later the same day still honours them) but the routing never runs them.
pub fn is_known(times: Option<&GrantTimes>, id: &str) -> bool {
    times.is_some_and(|t| t.get(id).is_some())
}

/// A time's label as the ward reads it.
pub fn label_of(times: Option<&GrantTimes>, id: Option<&str>) -> String {
    match id {
        None => FREE_TIME_LABEL.to_string(),
        Some(id) => times
            .and_then(|t| t.get(id))
            .map(|t| t.label.clone())
            .unwrap_or_else(|| FREE_TIME_LABEL.to_string()),
    }
}

/// The SITE apps an *only these* time lists — rendered per site as a
/// resolver-pinned window by the learning-app machinery, and the reason the
/// sweep demotes Chromium to a runtime while the time is in force.
pub fn time_sites(t: &NamedTime) -> Vec<LearningApp> {
    if t.posture != TimePosture::OnlyThese {
        return Vec::new();
    }
    t.sites
        .iter()
        .filter(|a| a.kind == LearningAppKind::Site)
        .cloned()
        .collect()
}

/// "HH:MM" for `instant` in `tz`, when it falls on the same local day as
/// `now`; `None` otherwise (a time running past midnight has no "until").
fn hhmm_today(tz: chrono_tz::Tz, now: i64, instant: i64) -> Option<String> {
    let at = tz.timestamp_opt(instant, 0).single()?;
    let today = tz.timestamp_opt(now, 0).single()?;
    (at.date_naive() == today.date_naive()).then(|| format!("{:02}:{:02}", at.hour(), at.minute()))
}

/// What follows the running time: a time's label, or `None` for "then
/// locked". Only a SPAN end can be followed by anything — a time that ends
/// because the budget does is followed by a lock. A following time that
/// counts against a budget already spent is a lock too.
pub fn following_label(
    s: &TimesState,
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
) -> Option<String> {
    let end = s.span_end?;
    let sched = schedule?;
    match charter_spine::times::evaluate_times(sched, times, end) {
        TimedStatus::Open { time, .. } => {
            let counts = match time.as_deref() {
                None => true,
                Some(id) => times
                    .and_then(|t| t.get(id))
                    .is_some_and(NamedTime::counts_against_budget),
            };
            if counts && s.remaining.budget_secs == 0 {
                // Spent already, and the core's budget lock would close it.
                // (A non-counting current time cannot spend more of it.)
                return None;
            }
            Some(label_of(times, time.as_deref()))
        }
        _ => None,
    }
}

/// "Locked until Learning time at 09:00" (tomorrow suffixed), or "Locked
/// today" when no runnable window starts today or tomorrow.
pub fn locked_until_title(
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    now: i64,
) -> String {
    match schedule.and_then(|s| charter_spine::times::next_window(s, times, now)) {
        Some(nw) => {
            let label = label_of(times, nw.time.as_deref());
            if nw.day_offset == 0 {
                format!("Locked until {label} at {}", nw.start_hhmm)
            } else {
                format!("Locked until {label} at {} tomorrow", nw.start_hhmm)
            }
        }
        None => "Locked today".to_string(),
    }
}

/// The lock card's headline/detail from the times layer, or `None` to keep
/// the ordinary lock copy. Only spoken when the child has named times or a
/// tagged window — a pre-times family keeps its exact lock card.
pub fn lock_copy(
    s: &TimesState,
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    now: i64,
) -> Option<(String, Option<String>)> {
    if !s.remaining.locked {
        return None;
    }
    let has_times = times.is_some_and(|t| !t.times.is_empty());
    if !has_times && !s.in_play {
        return None;
    }
    match &s.verdict {
        Verdict::Locked(TimesLock::UnknownTime(_)) => Some((
            locked_until_title(schedule, times, now),
            Some(UNKNOWN_TIME_DETAIL.to_string()),
        )),
        Verdict::Locked(TimesLock::InventoryUnknown(id)) => Some((
            format!("Getting {} ready", label_of(times, Some(id))),
            Some(
                "Kintrinsic is still checking which apps are on this computer. This opens in a moment."
                    .to_string(),
            ),
        )),
        _ => match s.remaining.reason {
            // A dormant device keeps its own honest headline (it has no
            // allowed hours to be "until"); see `lock_info`.
            Some(LockReason::Schedule)
                if charter_spine::enforcer_runtime::schedule_is_dormant(schedule) =>
            {
                None
            }
            Some(LockReason::Schedule) => Some((locked_until_title(schedule, times, now), None)),
            // Out of budget: a named time that does not count against it can
            // still run today, and that is the thing worth saying.
            Some(LockReason::Budget) => {
                let nw = schedule.and_then(|sc| charter_spine::times::next_window(sc, times, now))?;
                let id = nw.time.as_deref()?;
                let t = times?.get(id)?;
                (nw.day_offset == 0 && !t.counts_against_budget()).then(|| {
                    (
                        format!("Locked until {} at {}", t.label, nw.start_hhmm),
                        None,
                    )
                })
            }
            _ => None,
        },
    }
}

/// The default kind of time an ask names: inside a window now ⇒ that window's
/// time (or the named time running off its pool); else the time whose window
/// most recently ended today, else the next one up today, else Free.
pub fn default_ask(
    s: &TimesState,
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    now: i64,
) -> String {
    if let Verdict::Named { id, .. } = &s.verdict {
        return id.clone();
    }
    match &s.timed {
        TimedStatus::Open { time: Some(id), .. } if is_known(times, id) => id.clone(),
        TimedStatus::Open { time: None, .. } => FREE_TIME_CHOICE.to_string(),
        _ => schedule
            .and_then(|sc| charter_spine::times::default_ask_time(sc, times, now))
            .unwrap_or_else(|| FREE_TIME_CHOICE.to_string()),
    }
}

/// What an *only these* time allows, by the names the ward knows: each app's
/// inventory label (falling back to the identity's last path segment), then
/// each site's label.
pub fn allowed_labels(t: &NamedTime, inventory: &[AppRef]) -> Vec<String> {
    if t.posture != TimePosture::OnlyThese {
        return Vec::new();
    }
    let apps = t.apps.iter().map(|pkg| {
        inventory
            .iter()
            .find(|a| &a.pkg == pkg)
            .map(|a| a.label.clone())
            .unwrap_or_else(|| pkg.rsplit('/').next().unwrap_or(pkg).to_string())
    });
    let sites = t.sites.iter().map(|s| s.label.clone());
    apps.chain(sites).collect()
}

/// The ward's view of times for one child. `None` when the child has no named
/// time and no tagged window — every pre-times readout stays byte-identical.
pub fn times_view(
    s: &TimesState,
    schedule: Option<&GrantSchedule>,
    times: Option<&GrantTimes>,
    inventory: &[AppRef],
    tz: chrono_tz::Tz,
    now: i64,
) -> Option<TimesView> {
    let has_times = times.is_some_and(|t| !t.times.is_empty());
    if !has_times && !s.in_play {
        return None;
    }
    let mut v = TimesView::default();
    if s.remaining.locked {
        if let Some((title, detail)) = lock_copy(s, schedule, times, now) {
            v.lock_title = Some(title);
            v.lock_detail = detail;
        }
    } else {
        let named = active_time(&s.verdict, times);
        v.active = named.map(|t| t.id.clone());
        v.label = named
            .map(|t| t.label.clone())
            .unwrap_or_else(|| FREE_TIME_LABEL.to_string());
        if s.remaining.effective_secs >= 0 {
            v.until = hhmm_today(tz, now, now + s.remaining.effective_secs);
            v.then = following_label(s, schedule, times);
        }
        v.allowed = named
            .map(|t| allowed_labels(t, inventory))
            .unwrap_or_default();
    }
    if has_times {
        v.ask.push(TimeChoice {
            id: FREE_TIME_CHOICE.to_string(),
            label: FREE_TIME_LABEL.to_string(),
        });
        for t in times.map(|t| t.times.as_slice()).unwrap_or_default() {
            v.ask.push(TimeChoice {
                id: t.id.clone(),
                label: t.label.clone(),
            });
        }
        v.ask_default = Some(default_ask(s, schedule, times, now));
    }
    Some(v)
}

/// Edge state for the times layer's own 10-/1-minute warnings, per child.
/// Each fires once per running time; a change of time re-arms both, and so
/// does the time-left climbing back (a top-up) past the threshold.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WarnTracker {
    key: Option<String>,
    ten: bool,
    one: bool,
}

impl WarnTracker {
    /// `key` names the running time (its id, or [`FREE_TIME_CHOICE`]);
    /// `left` is the seconds until it ends (`< 0` = it does not).
    pub fn observe(&mut self, key: &str, left: i64) -> Option<WarnLevel> {
        if self.key.as_deref() != Some(key) {
            *self = WarnTracker {
                key: Some(key.to_string()),
                ten: false,
                one: false,
            };
        }
        if left < 0 {
            return None;
        }
        // Re-arm on a top-up, with a minute of hysteresis so a figure
        // hovering at the line cannot fire twice.
        if left > 11 * 60 {
            self.ten = false;
        }
        if left > 2 * 60 {
            self.one = false;
        }
        if left == 0 {
            return None;
        }
        if left <= 60 && !self.one {
            self.one = true;
            self.ten = true;
            return Some(WarnLevel::One);
        }
        if left <= 10 * 60 && !self.ten {
            self.ten = true;
            return Some(WarnLevel::Ten);
        }
        None
    }

    /// Forget the running time (the child is locked, or not in play): the
    /// next time that runs warns afresh.
    pub fn reset(&mut self) {
        *self = WarnTracker::default();
    }
}

/// The notification for a times warning: names the time and what follows.
pub fn warning_text(
    level: WarnLevel,
    label: &str,
    then: Option<&str>,
    left: i64,
) -> (String, String) {
    let summary = format!("Kintrinsic — {label} is nearly over");
    let next = match then {
        Some(t) => format!("then {t}"),
        None => "then locked".to_string(),
    };
    let body = match level {
        WarnLevel::Ten => {
            let m = ((left.max(0) + 59) / 60).max(1);
            format!("{m} minutes left of {label} — {next}.")
        }
        WarnLevel::One if then.is_none() => {
            format!("1 minute left of {label} — {next}. Save your game.")
        }
        WarnLevel::One => format!("1 minute left of {label} — {next}."),
    };
    (summary, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_schedule::{GrantScheduleWindow, WeeklySchedule};

    // Mon 2026-06-29 00:00:00 UTC.
    const MON: i64 = 1_782_691_200;
    const H: i64 = 3600;

    fn win(start: &str, end: &str, time: Option<&str>) -> GrantScheduleWindow {
        GrantScheduleWindow {
            start: start.into(),
            end: end.into(),
            time: time.map(String::from),
        }
    }

    /// Monday: Learning 09:00–12:00, Free 16:00–19:00 carved by Chores
    /// 17:00–18:00 (an unknown or known tag, per test).
    fn sched(mon: Vec<GrantScheduleWindow>) -> GrantSchedule {
        GrantSchedule {
            v: 1,
            tz: "Etc/UTC".into(),
            paused: None,
            weekly: WeeklySchedule {
                mon: Some(mon),
                ..Default::default()
            },
            overrides: None,
            issued_at: 1,
        }
    }

    fn times(json: &str) -> GrantTimes {
        GrantTimes::parse_or_absent(json).expect("valid times clause")
    }

    fn learning_only_these(counts: bool) -> GrantTimes {
        times(&format!(
            r#"{{"v":1,"issuedAt":1,"times":[{{"id":"learning","label":"Learning time","posture":"onlyThese","apps":["/usr/bin/gcompris"],"countsAgainstBudget":{counts}}}]}}"#
        ))
    }

    fn chores_except() -> GrantTimes {
        times(
            r#"{"v":1,"issuedAt":1,"times":[{"id":"chores","label":"Chores","posture":"everythingExcept","blockApps":["/usr/bin/minecraft"]}]}"#,
        )
    }

    fn open_core(effective: i64, budget: i64) -> Remaining {
        Remaining {
            effective_secs: effective,
            schedule_secs: effective,
            budget_secs: budget,
            extension_secs: 0,
            locked: false,
            reason: None,
            next_open_secs: None,
            budget_day_secs: budget,
            budget_week_secs: -1,
        }
    }

    fn locked_core(reason: LockReason) -> Remaining {
        Remaining {
            effective_secs: 0,
            schedule_secs: if reason == LockReason::Schedule {
                0
            } else {
                3600
            },
            budget_secs: if reason == LockReason::Budget { 0 } else { -1 },
            extension_secs: 0,
            locked: true,
            reason: Some(reason),
            next_open_secs: None,
            budget_day_secs: 0,
            budget_week_secs: -1,
        }
    }

    fn day() -> GrantSchedule {
        sched(vec![
            win("09:00", "12:00", Some("learning")),
            win("16:00", "19:00", None),
            win("17:00", "18:00", Some("chores")),
        ])
    }

    // ---- unknown tag ------------------------------------------------------

    #[test]
    fn an_unknown_tag_locks_its_span_even_though_the_core_runs_it_as_free() {
        let s = day();
        // No times clause at all: both tagged spans lock.
        let st = resolve(
            Some(&s),
            None,
            &open_core(3 * H, -1),
            true,
            None,
            MON + 10 * H,
        );
        assert_eq!(
            st.verdict,
            Verdict::Locked(TimesLock::UnknownTime("learning".into()))
        );
        assert!(st.remaining.locked);
        assert_eq!(st.remaining.reason, Some(LockReason::Schedule));
        // A times clause that defines only `learning`: the chores carve locks.
        let t = learning_only_these(false);
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(H, -1),
            true,
            None,
            MON + 17 * H + 60,
        );
        assert_eq!(
            st.verdict,
            Verdict::Locked(TimesLock::UnknownTime("chores".into()))
        );
        // …and the free span either side of it runs as Free.
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(3 * H, -1),
            true,
            None,
            MON + 16 * H,
        );
        assert_eq!(
            st.verdict,
            Verdict::Core {
                span_end: Some(MON + 17 * H)
            }
        );
        assert!(!st.remaining.locked);
        assert_eq!(
            st.remaining.effective_secs, H,
            "Free is cut short by the carve"
        );
    }

    #[test]
    fn an_unknown_tag_is_not_opened_by_a_named_pool() {
        let s = day();
        let t = learning_only_these(false);
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(H, -1),
            true,
            Some(("learning", 1800)),
            MON + 17 * H + 60,
        );
        assert_eq!(
            st.verdict,
            Verdict::Locked(TimesLock::UnknownTime("chores".into()))
        );
    }

    #[test]
    fn an_unparseable_times_clause_is_absent_and_locks_every_tagged_window() {
        assert!(parse_times(Some(r#"{"v":2,"issuedAt":1,"times":[]}"#)).is_none());
        assert!(parse_times(Some("garbage")).is_none());
        let st = resolve(
            Some(&day()),
            None,
            &open_core(3 * H, -1),
            true,
            None,
            MON + 9 * H,
        );
        assert!(matches!(
            st.verdict,
            Verdict::Locked(TimesLock::UnknownTime(_))
        ));
    }

    // ---- inside a named window -------------------------------------------

    #[test]
    fn a_non_counting_window_runs_whatever_the_budget_says() {
        let t = learning_only_these(false);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Budget),
            true,
            None,
            MON + 10 * H,
        );
        assert!(matches!(
            st.verdict,
            Verdict::Named { ref id, from_pool: false, counts: false, .. } if id == "learning"
        ));
        assert!(!st.remaining.locked);
        assert_eq!(
            st.remaining.effective_secs,
            2 * H,
            "windowLeftSecs from span_end"
        );
    }

    #[test]
    fn a_counting_window_with_the_budget_spent_falls_through_to_the_pools() {
        let t = learning_only_these(true);
        // Nothing in any pool: the budget lock stands.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Budget),
            true,
            None,
            MON + 10 * H,
        );
        assert_eq!(st.verdict, Verdict::Core { span_end: None });
        assert!(st.remaining.locked);
    }

    #[test]
    fn a_stand_down_or_malformed_clause_outranks_every_named_time() {
        let t = learning_only_these(false);
        for reason in [LockReason::StandDown, LockReason::Malformed] {
            let st = resolve(
                Some(&day()),
                Some(&t),
                &locked_core(reason),
                true,
                Some(("learning", 3600)),
                MON + 10 * H,
            );
            assert_eq!(st.verdict, Verdict::Core { span_end: None }, "{reason:?}");
            assert!(st.remaining.locked);
            assert_eq!(st.remaining.reason, Some(reason));
        }
    }

    // ---- inventory unknown --------------------------------------------------

    #[test]
    fn an_only_these_time_locks_while_the_inventory_is_unknown() {
        let t = learning_only_these(false);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(3 * H, -1),
            false,
            None,
            MON + 10 * H,
        );
        assert_eq!(
            st.verdict,
            Verdict::Locked(TimesLock::InventoryUnknown("learning".into()))
        );
        assert!(st.remaining.locked);
        assert_eq!(st.remaining.reason, Some(LockReason::Schedule));
        // Known (even if empty) runs it.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(3 * H, -1),
            true,
            None,
            MON + 10 * H,
        );
        assert!(matches!(st.verdict, Verdict::Named { .. }));
        // An everythingExcept time does not depend on the inventory.
        let t = chores_except();
        let s = sched(vec![win("09:00", "12:00", Some("chores"))]);
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(3 * H, -1),
            false,
            None,
            MON + 10 * H,
        );
        assert!(matches!(st.verdict, Verdict::Named { .. }));
    }

    #[test]
    fn a_pool_run_only_these_time_also_locks_while_the_inventory_is_unknown() {
        let t = learning_only_these(false);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Schedule),
            false,
            Some(("learning", 1800)),
            MON + 20 * H,
        );
        assert_eq!(
            st.verdict,
            Verdict::Locked(TimesLock::InventoryUnknown("learning".into()))
        );
    }

    // ---- fall-through order -------------------------------------------------

    #[test]
    fn free_s_own_pool_runs_first_outside_every_window() {
        // The core is UNLOCKED at 20:00 — Free's schedule pool is running
        // (step 1). A named pool waiting behind it is not touched.
        let t = learning_only_these(false);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(900, -1),
            true,
            Some(("learning", 1800)),
            MON + 20 * H,
        );
        assert_eq!(st.verdict, Verdict::Core { span_end: None });
        assert_eq!(st.remaining, open_core(900, -1));
        assert_eq!(
            burning_pool(&st.verdict),
            None,
            "no named pool burns under Free"
        );
    }

    #[test]
    fn then_the_most_recent_named_pool_runs_as_that_named_time() {
        let t = learning_only_these(false);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Schedule),
            true,
            Some(("learning", 1800)),
            MON + 20 * H,
        );
        assert_eq!(
            st.verdict,
            Verdict::Named {
                id: "learning".into(),
                from_pool: true,
                counts: false,
                span_end: MON + 20 * H + 1800,
                ends_at: MON + 20 * H + 1800,
                span_limited: true,
            }
        );
        assert_eq!(burning_pool(&st.verdict), Some("learning"));
        assert_eq!(st.remaining.effective_secs, 1800);
        assert_eq!(
            st.remaining.schedule_secs, 1800,
            "a pool-run span ends with the pool"
        );
        assert!(!st.remaining.locked);
    }

    #[test]
    fn a_budget_spent_free_window_falls_through_to_a_named_pool() {
        // The design's example: 30 minutes of Learning time given at 17:30 to
        // a child whose Free budget ran out inside 16:00–19:00.
        let s = sched(vec![win("16:00", "19:00", None)]);
        let t = learning_only_these(false);
        let st = resolve(
            Some(&s),
            Some(&t),
            &locked_core(LockReason::Budget),
            true,
            Some(("learning", 1800)),
            MON + 17 * H + 1800,
        );
        assert!(
            matches!(st.verdict, Verdict::Named { ref id, from_pool: true, .. } if id == "learning")
        );
    }

    #[test]
    fn nothing_left_in_any_pool_keeps_the_lock() {
        let t = learning_only_these(false);
        for pool in [None, Some(("learning", 0))] {
            let st = resolve(
                Some(&day()),
                Some(&t),
                &locked_core(LockReason::Schedule),
                true,
                pool,
                MON + 20 * H,
            );
            assert_eq!(st.verdict, Verdict::Core { span_end: None });
            assert!(st.remaining.locked);
        }
    }

    #[test]
    fn a_schedule_take_back_inside_an_open_free_window_is_not_bought_out_by_a_pool() {
        let s = sched(vec![win("16:00", "19:00", None)]);
        let t = learning_only_these(false);
        let st = resolve(
            Some(&s),
            Some(&t),
            &locked_core(LockReason::Schedule),
            true,
            Some(("learning", 1800)),
            MON + 17 * H,
        );
        assert_eq!(st.verdict, Verdict::Core { span_end: None });
    }

    #[test]
    fn a_free_span_running_into_outside_every_window_keeps_the_core_figure() {
        // Free 16:00–19:00 alone: at 18:00 the core says 1h + any schedule
        // pool, which is the truth — no clamp.
        let s = sched(vec![
            win("16:00", "19:00", None),
            win("09:00", "10:00", Some("learning")),
        ]);
        let t = learning_only_these(false);
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(90 * 60, -1),
            true,
            None,
            MON + 18 * H,
        );
        assert_eq!(st.verdict, Verdict::Core { span_end: None });
        assert_eq!(st.remaining.effective_secs, 90 * 60);
    }

    // ---- charging -----------------------------------------------------------

    #[test]
    fn a_non_counting_time_charges_nothing_and_a_counting_one_charges_as_before() {
        let named = |counts| Verdict::Named {
            id: "learning".into(),
            from_pool: false,
            counts,
            span_end: MON,
            ends_at: MON,
            span_limited: true,
        };
        assert_eq!(
            charge_activity(Activity::Active, &named(false)),
            Activity::Idle
        );
        assert!(!charge_activity(Activity::Active, &named(false)).counts());
        assert_eq!(
            charge_activity(Activity::Active, &named(true)),
            Activity::Active
        );
        assert_eq!(
            charge_activity(Activity::Active, &Verdict::Core { span_end: None }),
            Activity::Active
        );
        // A screen already off stays off.
        assert_eq!(
            charge_activity(Activity::Locked, &named(false)),
            Activity::Locked
        );
    }

    #[test]
    fn a_stand_down_grace_caps_a_running_named_time() {
        let t = learning_only_these(false);
        let mut st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(60, -1),
            true,
            None,
            MON + 10 * H,
        );
        apply_stand_down(&mut st, Some(60), MON + 10 * H);
        assert!(matches!(
            st.verdict,
            Verdict::Named { span_limited: false, ends_at, .. } if ends_at == MON + 10 * H + 60
        ));
        assert_eq!(st.remaining.effective_secs, 60);
        assert_eq!(st.span_end, None, "then locked, not then Free time");
    }

    #[test]
    fn a_counting_window_ends_with_the_budget() {
        let t = learning_only_these(true);
        let st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(900, 900),
            true,
            None,
            MON + 10 * H,
        );
        assert!(matches!(
            st.verdict,
            Verdict::Named { counts: true, span_limited: false, ends_at, .. } if ends_at == MON + 10 * H + 900
        ));
        assert_eq!(st.span_end, None, "budget-limited: then locked");
        // windowLeftSecs is still the span (to 12:00); only effective is capped.
        assert_eq!(st.remaining.schedule_secs, 2 * H);
        assert_eq!(st.remaining.effective_secs, 900);
    }

    // ---- decisions ------------------------------------------------------------

    fn decision(locked: bool, reason: Option<LockReason>) -> ChildDecision {
        ChildDecision {
            uid: 1001,
            active: true,
            locked,
            reason,
            effects: vec![EnforcerEffect::Warn(WarnLevel::Ten)],
            source: charter_spine::child_policy::PolicySource::Guardian,
        }
    }

    #[test]
    fn the_verdict_overrides_the_core_decision_in_both_directions() {
        let t = learning_only_these(false);
        // Unknown tag: the open core decision is LOCKED.
        let st = resolve(
            Some(&day()),
            None,
            &open_core(3 * H, -1),
            true,
            None,
            MON + 10 * H,
        );
        let mut d = decision(false, None);
        apply_to_decision(&mut d, &st);
        assert!(d.locked);
        assert_eq!(d.reason, Some(LockReason::Schedule));
        assert!(
            d.effects.is_empty(),
            "core warnings dropped while times are in play"
        );
        // Non-counting window over a spent budget: the locked core decision OPENS.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Budget),
            true,
            None,
            MON + 10 * H,
        );
        let mut d = decision(true, Some(LockReason::Budget));
        apply_to_decision(&mut d, &st);
        assert!(!d.locked);
        assert_eq!(d.reason, None);
    }

    #[test]
    fn a_family_with_no_times_keeps_the_core_decision_and_warnings_untouched() {
        let s = sched(vec![win("16:00", "19:00", None)]);
        let core = open_core(H, 2 * H);
        let st = resolve(Some(&s), None, &core, true, None, MON + 18 * H);
        assert!(!st.in_play);
        assert_eq!(st.remaining, core);
        let mut d = decision(false, None);
        apply_to_decision(&mut d, &st);
        assert_eq!(d, decision(false, None));
    }

    // ---- end to end through the live enforcer ------------------------------------

    fn enforcer_with(
        schedule: GrantSchedule,
        budget_daily: Option<u16>,
        now: i64,
    ) -> charter_spine::multi_child::MultiChildEnforcer {
        use charter_spine::child_policy::{EffectivePolicy, PolicySource};
        let budget = budget_daily.map(|d| {
            serde_json::from_value::<charter_schedule::GrantBudget>(serde_json::json!({
                "v": 1, "tz": "Etc/UTC", "dailyMinutes": d, "issuedAt": 1
            }))
            .expect("budget")
        });
        let mut multi = charter_spine::multi_child::MultiChildEnforcer::new();
        multi.sync(
            &[(
                UID,
                EffectivePolicy {
                    schedule: Some(schedule),
                    budget,
                    learning: None,
                    source: PolicySource::Guardian,
                },
            )],
            now,
            |_| (None, None),
        );
        multi
    }

    const UID: u32 = 1001;

    #[test]
    fn a_typed_grant_unlocks_out_of_window_as_its_time_and_an_unknown_one_does_not() {
        let s = sched(vec![win("16:00", "19:00", None)]);
        let t = learning_only_these(false);
        let now = MON + 20 * H;
        let mut multi = enforcer_with(s.clone(), None, now);
        multi.tick(Some(UID), now, 0);
        // Locked outside every window, nothing in any pool.
        let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, now).unwrap();
        assert!(st.remaining.locked);
        // A grant for a time this device does not define enacts NOTHING…
        assert!(multi.apply_time_extension(UID, now, "req-chores", 30, "chores"));
        let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, now).unwrap();
        assert!(st.remaining.locked, "unknown time id ⇒ no extra time");
        // …and is not run as Free either: Free's own pool is untouched.
        assert_eq!(multi.remaining(UID, now).unwrap().extension_secs, 0);
        // The routed typed grant runs the device AS Learning time.
        let p = pending(None, None, Some("learning"));
        let PendingRoute::Time(id) = pending_route(&p) else {
            panic!("typed grant must route to its time");
        };
        assert!(multi.apply_time_extension(UID, now, &p.req_id, p.minutes, id));
        let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, now).unwrap();
        assert!(
            matches!(st.verdict, Verdict::Named { ref id, from_pool: true, .. } if id == "learning")
        );
        assert_eq!(st.remaining.effective_secs, 30 * 60);
        // It burns wall clock while it runs, and locks again when spent.
        multi.burn_time_pool(UID, now, "learning", 30 * 60);
        let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, now + 30 * 60).unwrap();
        assert!(st.remaining.locked);
    }

    #[test]
    fn usage_in_a_non_counting_time_charges_neither_the_budget_nor_a_bucket() {
        let s = sched(vec![
            win("09:00", "12:00", Some("learning")),
            win("16:00", "19:00", None),
        ]);
        let t = learning_only_these(false);
        let now = MON + 9 * H;
        let mut multi = enforcer_with(s.clone(), Some(60), now);
        multi.tick(Some(UID), now, 0);
        let open: Vec<String> = vec!["play".into()];
        for step in 1..=10 {
            let at = now + step * 60;
            let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, at).unwrap();
            let charge = charge_activity(Activity::Active, &st.verdict);
            multi.tick_open_with(
                charge,
                Some(UID),
                at,
                60,
                charter_schedule::Bucket::Screen,
                Some(&open),
                false,
            );
        }
        assert_eq!(
            multi.used_today(UID, now + 600),
            Some(0),
            "no screen budget charged"
        );
        assert_eq!(
            multi.app_bucket_today(UID, "play", now + 600),
            Some(0),
            "no bucket charged"
        );
        // The same ten minutes in Free time charge as always.
        let free_at = MON + 16 * H;
        for step in 1..=10 {
            let at = free_at + step * 60;
            let st = state_for(&multi, Some(&s), Some(&t), true, None, UID, at).unwrap();
            let charge = charge_activity(Activity::Active, &st.verdict);
            multi.tick_open_with(
                charge,
                Some(UID),
                at,
                60,
                charter_schedule::Bucket::Screen,
                Some(&open),
                false,
            );
        }
        assert_eq!(multi.used_today(UID, free_at + 600), Some(600));
    }

    // ---- inbox drain ------------------------------------------------------------

    fn pending(
        dim: Option<Dimension>,
        bucket: Option<&str>,
        time: Option<&str>,
    ) -> PendingExtension {
        PendingExtension {
            req_id: "r".into(),
            minutes: 30,
            dim,
            bucket_id: bucket.map(String::from),
            time_id: time.map(String::from),
            at: MON,
        }
    }

    #[test]
    fn a_typed_grant_is_routed_to_its_time_not_dropped() {
        assert_eq!(
            pending_route(&pending(None, None, Some("learning"))),
            PendingRoute::Time("learning")
        );
        assert_eq!(
            pending_route(&pending(None, Some("play"), Some("learning"))),
            PendingRoute::Bucket("play")
        );
        assert_eq!(
            pending_route(&pending(Some(Dimension::Schedule), None, None)),
            PendingRoute::Dimension(Dimension::Schedule)
        );
        assert_eq!(
            pending_route(&pending(None, None, None)),
            PendingRoute::Nothing
        );
    }

    #[test]
    fn a_pool_for_an_unknown_time_enacts_no_extra_time() {
        let t = learning_only_these(false);
        assert!(is_known(Some(&t), "learning"));
        assert!(!is_known(Some(&t), "chores"));
        assert!(!is_known(None, "learning"));
    }

    // ---- ward-facing copy ---------------------------------------------------------

    #[test]
    fn the_lock_card_names_the_next_time() {
        let t = learning_only_these(false);
        // 07:00: Learning at 09:00 is next.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Schedule),
            true,
            None,
            MON + 7 * H,
        );
        let (title, detail) = lock_copy(&st, Some(&day()), Some(&t), MON + 7 * H).unwrap();
        assert_eq!(title, "Locked until Learning time at 09:00");
        assert_eq!(detail, None);
        // Unknown carve at 17:00: next runnable is Free at 18:00.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &open_core(H, -1),
            true,
            None,
            MON + 17 * H + 60,
        );
        let (title, detail) = lock_copy(&st, Some(&day()), Some(&t), MON + 17 * H + 60).unwrap();
        assert_eq!(title, "Locked until Free time at 18:00");
        assert_eq!(detail.as_deref(), Some(UNKNOWN_TIME_DETAIL));
        // After 19:00 on a Monday-only schedule nothing runs tomorrow.
        let st = resolve(
            Some(&day()),
            Some(&t),
            &locked_core(LockReason::Schedule),
            true,
            None,
            MON + 20 * H,
        );
        assert_eq!(
            lock_copy(&st, Some(&day()), Some(&t), MON + 20 * H)
                .unwrap()
                .0,
            "Locked today"
        );
    }

    #[test]
    fn the_ask_defaults_to_the_window_now_then_the_last_ended_then_the_next() {
        let t = learning_only_these(false);
        let s = sched(vec![
            win("09:00", "12:00", Some("learning")),
            win("16:00", "19:00", None),
        ]);
        let at = |now: i64, core: &Remaining| {
            let st = resolve(Some(&s), Some(&t), core, true, None, now);
            default_ask(&st, Some(&s), Some(&t), now)
        };
        // Inside Learning, locked or not: Learning.
        assert_eq!(
            at(MON + 10 * H, &locked_core(LockReason::Budget)),
            "learning"
        );
        // 13:00, outside: Learning most recently ended.
        assert_eq!(
            at(MON + 13 * H, &locked_core(LockReason::Schedule)),
            "learning"
        );
        // 07:00: nothing ended yet, Learning is next up.
        assert_eq!(
            at(MON + 7 * H, &locked_core(LockReason::Schedule)),
            "learning"
        );
        // Inside Free with the budget spent: Free.
        assert_eq!(
            at(MON + 17 * H, &locked_core(LockReason::Budget)),
            FREE_TIME_CHOICE
        );
    }

    #[test]
    fn the_view_says_which_time_until_when_what_follows_and_what_is_allowed() {
        let t = learning_only_these(false);
        let inv = vec![AppRef {
            pkg: "/usr/bin/gcompris".into(),
            label: "GCompris".into(),
            user_installed: None,
            hidden: None,
        }];
        let s = sched(vec![
            win("09:00", "12:00", Some("learning")),
            win("12:00", "13:00", None),
        ]);
        let st = resolve(
            Some(&s),
            Some(&t),
            &open_core(4 * H, -1),
            true,
            None,
            MON + 10 * H,
        );
        let v = times_view(&st, Some(&s), Some(&t), &inv, chrono_tz::UTC, MON + 10 * H).unwrap();
        assert_eq!(v.active.as_deref(), Some("learning"));
        assert_eq!(v.label, "Learning time");
        assert_eq!(v.until.as_deref(), Some("12:00"));
        assert_eq!(v.then.as_deref(), Some("Free time"));
        assert_eq!(v.allowed, vec!["GCompris".to_string()]);
        assert_eq!(
            v.ask.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec![FREE_TIME_CHOICE, "learning"]
        );
        assert_eq!(v.ask_default.as_deref(), Some("learning"));
        // No times and no tags: no view at all (pre-times byte-identical).
        let plain = sched(vec![win("16:00", "19:00", None)]);
        let st = resolve(
            Some(&plain),
            None,
            &open_core(H, -1),
            true,
            None,
            MON + 18 * H,
        );
        assert_eq!(
            times_view(&st, Some(&plain), None, &inv, chrono_tz::UTC, MON + 18 * H),
            None
        );
    }

    #[test]
    fn warnings_fire_once_per_time_name_it_and_say_what_follows() {
        let mut w = WarnTracker::default();
        assert_eq!(w.observe("learning", 20 * 60), None);
        assert_eq!(w.observe("learning", 10 * 60), Some(WarnLevel::Ten));
        assert_eq!(w.observe("learning", 9 * 60), None);
        assert_eq!(w.observe("learning", 60), Some(WarnLevel::One));
        assert_eq!(w.observe("learning", 30), None);
        // A new time re-arms.
        assert_eq!(w.observe(FREE_TIME_CHOICE, 5 * 60), Some(WarnLevel::Ten));
        // A top-up past the line re-arms too.
        assert_eq!(w.observe(FREE_TIME_CHOICE, 30 * 60), None);
        assert_eq!(w.observe(FREE_TIME_CHOICE, 9 * 60), Some(WarnLevel::Ten));
        let (summary, body) = warning_text(WarnLevel::Ten, "Learning time", Some("Free time"), 600);
        assert_eq!(summary, "Kintrinsic — Learning time is nearly over");
        assert_eq!(body, "10 minutes left of Learning time — then Free time.");
        let (_, body) = warning_text(WarnLevel::One, "Free time", None, 60);
        assert_eq!(
            body,
            "1 minute left of Free time — then locked. Save your game."
        );
    }

    #[test]
    fn only_these_sites_are_the_time_s_site_apps() {
        let t = times(
            r#"{"v":1,"issuedAt":1,"times":[{"id":"learning","label":"Learning time","posture":"onlyThese","sites":[{"id":"khan-academy","label":"Khan Academy","kind":"site","url":"https://www.khanacademy.org/","domains":["khanacademy.org"]}]}]}"#,
        );
        let sites = time_sites(t.get("learning").unwrap());
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].id, "khan-academy");
        assert!(time_sites(chores_except().get("chores").unwrap()).is_empty());
    }
}
