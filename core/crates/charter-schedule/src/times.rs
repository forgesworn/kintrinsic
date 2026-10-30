//! Times-aware schedule evaluation (§Times): which TIME is the device in now?
//!
//! The classic evaluators ([`evaluate_grant_schedule`](crate::evaluate_grant_schedule)
//! and friends) read only `start`/`end` and therefore run a tagged window as
//! Free time — which is exactly what a pre-times ward does, and why they are
//! left untouched. Anything that must honour tags calls [`evaluate_times`].
//!
//! This module is deliberately ignorant of the `times` clause itself: it takes
//! a `known` predicate ("does the device's `times` clause define this id?") so
//! this crate keeps no dependency on `charter-proto`. `charter_spine::times`
//! wraps it with the `Option<&GrantTimes>` signature the wardens call.
//!
//! # Resolution at an instant
//!
//! 1. Take today's windows (the override for the date, else the weekly day).
//! 2. Of the **tagged** windows covering the minute, the one with the earliest
//!    `start` wins (ties: array order). A tagged window beats every untagged
//!    one — it carves.
//! 3. Else an **untagged** window covering the minute ⇒ Free time.
//! 4. Else the minute is outside every window ⇒ locked.
//!
//! Only AFTER the winner is chosen is its tag looked up: a winning tag the
//! device does not know (no `times` clause at all, an unparseable one, or an
//! id it does not define) LOCKS that span. Checking known-ness first and
//! falling back to the overlapped untagged window would open the span as
//! Free — granting exactly what a named window exists to withhold.
//!
//! All of this is evaluated in the schedule's own tz, and every instant that
//! leaves this module is a real instant ([`local_wall_ts`]), so a DST day is
//! counted as it actually elapses.

use chrono::{DateTime, Datelike, Timelike};
use chrono_tz::Tz;

use crate::clause::{GrantSchedule, GrantScheduleWindow};
use crate::schedule_eval::local_wall_ts;

/// Why a locked span is locked. Every cause is `lockReason: 'schedule'` on the
/// wire (so a current guardian app parses it); the distinction is for the
/// ward-facing copy only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockCause {
    /// The schedule is `paused` (a dormant device).
    Paused,
    /// Outside every window.
    OutsideWindows,
    /// The winning window is tagged with an id the device does not know:
    /// "Nothing is set up for this time yet". Carries the tag.
    UnknownTime(String),
}

/// What the device is doing under its schedule right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimedStatus {
    /// No schedule constraint at all (empty schedule): Free time, never closes.
    Unbounded,
    /// Inside a window that can run. `time` is the named-time id, or `None`
    /// for Free time. `span_end` is the unix instant the contiguous span of
    /// that SAME time ends (not the winning window's own `end`: Free
    /// 16:00–19:00 carved by Learning 17:00–18:00 is Free until 17:00 at
    /// 16:30). The seconds-left readout (`windowLeftSecs`) is
    /// `span_end - now`; evaluate again at `span_end` for "what follows".
    Open { time: Option<String>, span_end: i64 },
    /// Locked. `seconds_to_open` is the distance to the next instant any
    /// window that can run opens (`None` if none within 8 days, or paused).
    Locked {
        cause: LockCause,
        seconds_to_open: Option<u64>,
    },
    /// The clause could not be read (unsupported version, bad tz). The
    /// enforcement layer fails SAFE on this — never open.
    Unparseable,
}

impl TimedStatus {
    /// Seconds from `now_unix` to the end of the span, if open. Saturated at
    /// zero.
    pub fn window_left_secs(&self, now_unix: i64) -> Option<u64> {
        match self {
            TimedStatus::Open { span_end, .. } => Some((span_end - now_unix).max(0) as u64),
            _ => None,
        }
    }
}

/// The next time a runnable span STARTS, for "Locked until Learning time at
/// 09:00".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextWindow {
    /// Named-time id, or `None` for Free time.
    pub time: Option<String>,
    /// The unix instant it starts.
    pub start: i64,
    /// Its start as the guardian wrote it, `"HH:MM"` in the schedule tz.
    pub start_hhmm: String,
    /// 0 = today, 1 = tomorrow (local calendar days).
    pub day_offset: u8,
}

struct Win<'a> {
    start: u32,
    end: u32,
    tag: Option<&'a str>,
}

#[derive(PartialEq)]
enum Label<'a> {
    Closed,
    Free,
    Time(&'a str),
    Unknown(&'a str),
}

impl Label<'_> {
    fn runnable(&self) -> bool {
        matches!(self, Label::Free | Label::Time(_))
    }
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    (h <= 23 && m <= 59).then_some(h * 60 + m)
}

/// Today's windows with their tags. An override for the date REPLACES the
/// weekly entry (an empty array = blocked). Windows that are malformed or not
/// `start < end` are dropped, as the classic parser drops them.
fn day_windows<'a>(sched: &'a GrantSchedule, date_key: &str, day: u32) -> Vec<Win<'a>> {
    let raw: &[GrantScheduleWindow] = match sched.overrides.as_ref().and_then(|o| o.get(date_key)) {
        Some(ovr) => ovr,
        None => match sched.weekly.for_day(day) {
            Some(w) => w,
            None => return Vec::new(),
        },
    };
    raw.iter()
        .filter_map(|w| {
            let (s, e) = (parse_hhmm(&w.start)?, parse_hhmm(&w.end)?);
            (s < e).then_some(Win {
                start: s,
                end: e,
                tag: w.time.as_deref(),
            })
        })
        .collect()
}

fn label_at<'a>(wins: &[Win<'a>], known: &dyn Fn(&str) -> bool, minute: u32) -> Label<'a> {
    // Tagged beats untagged; among tagged, earliest start, then array order
    // (`min_by_key` keeps the first of equals, and iteration is array order).
    let tagged = wins
        .iter()
        .filter(|w| w.tag.is_some() && minute >= w.start && minute < w.end)
        .min_by_key(|w| w.start);
    if let Some(w) = tagged {
        let id = w.tag.expect("filtered to tagged");
        return if known(id) {
            Label::Time(id)
        } else {
            Label::Unknown(id)
        };
    }
    if wins
        .iter()
        .any(|w| w.tag.is_none() && minute >= w.start && minute < w.end)
    {
        Label::Free
    } else {
        Label::Closed
    }
}

/// Minutes at which any window starts or ends, ascending — the only places
/// the resolved label can change.
fn boundaries(wins: &[Win<'_>]) -> Vec<u32> {
    let mut b: Vec<u32> = wins.iter().flat_map(|w| [w.start, w.end]).collect();
    b.sort_unstable();
    b.dedup();
    b
}

/// The first minute after `minute` at which the label differs from `cur`, or
/// 1440 (the end of the local day). Spans never merge across midnight: windows
/// are day-bound, so the day's last span ends at its last window's `end`.
fn span_end_minute(
    wins: &[Win<'_>],
    known: &dyn Fn(&str) -> bool,
    minute: u32,
    cur: &Label,
) -> u32 {
    boundaries(wins)
        .into_iter()
        .filter(|&b| b > minute)
        .find(|&b| label_at(wins, known, b) != *cur)
        .unwrap_or(1440)
}

fn hhmm(minute: u32) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

fn local_now(sched: &GrantSchedule, now_unix: i64) -> Option<(Tz, DateTime<Tz>)> {
    let tz: Tz = sched.tz.parse().ok()?;
    let now = DateTime::from_timestamp(now_unix, 0)?.with_timezone(&tz);
    Some((tz, now))
}

fn is_unbounded(sched: &GrantSchedule) -> bool {
    let has_overrides = sched.overrides.as_ref().is_some_and(|o| !o.is_empty());
    sched.weekly.is_empty() && !has_overrides
}

/// Which time is the device in at `now_unix`? See the module docs for the
/// resolution order; `known` answers whether the device's `times` clause
/// defines an id (always `false` when there is no usable clause — every tagged
/// window then locks and untagged windows are unaffected).
pub fn evaluate_times(
    sched: &GrantSchedule,
    known: &dyn Fn(&str) -> bool,
    now_unix: i64,
) -> TimedStatus {
    // Same ordering as the classic evaluator, for the same reasons: an unknown
    // schedule version reads as UNPARSEABLE before `paused` or any field.
    if !sched.is_supported_version() {
        return TimedStatus::Unparseable;
    }
    if sched.paused == Some(true) {
        return TimedStatus::Locked {
            cause: LockCause::Paused,
            seconds_to_open: None,
        };
    }
    let Some((tz, now)) = local_now(sched, now_unix) else {
        return TimedStatus::Unparseable;
    };
    if is_unbounded(sched) {
        return TimedStatus::Unbounded;
    }
    let day = now.weekday().num_days_from_sunday();
    let minute = now.hour() * 60 + now.minute();
    let date = now.date_naive();
    let wins = day_windows(sched, &date.format("%Y-%m-%d").to_string(), day);

    let label = label_at(&wins, known, minute);
    match label {
        Label::Free | Label::Time(_) => {
            let end = span_end_minute(&wins, known, minute, &label);
            let span_end = local_wall_ts(tz, date, end);
            TimedStatus::Open {
                time: match label {
                    Label::Time(id) => Some(id.to_string()),
                    _ => None,
                },
                // Saturated like the classic countdown: on a fall-back day the
                // EARLIER instant can already be behind us on the second pass,
                // and closing early is the fail-safe direction.
                span_end: span_end.max(now_unix),
            }
        }
        Label::Unknown(id) => TimedStatus::Locked {
            cause: LockCause::UnknownTime(id.to_string()),
            seconds_to_open: next_open_secs(sched, known, tz, now_unix),
        },
        Label::Closed => TimedStatus::Locked {
            cause: LockCause::OutsideWindows,
            seconds_to_open: next_open_secs(sched, known, tz, now_unix),
        },
    }
}

/// Seconds to the next instant a runnable label begins (today's later
/// boundaries, then up to 8 calendar days).
fn next_open_secs(
    sched: &GrantSchedule,
    known: &dyn Fn(&str) -> bool,
    tz: Tz,
    now_unix: i64,
) -> Option<u64> {
    let now = DateTime::from_timestamp(now_unix, 0)?.with_timezone(&tz);
    let today = now.date_naive();
    for day_offset in 0..8u64 {
        // CALENDAR days, not `now + 86400 * n` — see `next_open_secs` in the
        // classic evaluator.
        let probe = today.checked_add_days(chrono::Days::new(day_offset))?;
        let wins = day_windows(
            sched,
            &probe.format("%Y-%m-%d").to_string(),
            probe.weekday().num_days_from_sunday(),
        );
        for b in boundaries(&wins) {
            if label_at(&wins, known, b).runnable() {
                let open = local_wall_ts(tz, probe, b);
                if open > now_unix {
                    return Some((open - now_unix) as u64);
                }
            }
        }
    }
    None
}

/// The next runnable span to START after `now_unix`, today or tomorrow, for
/// the lock card's "Locked until Learning time at 09:00". `None` means nothing
/// starts today or tomorrow ("Locked today"); also `None` when the schedule is
/// paused, unbounded or unreadable.
///
/// Spans whose tag the device does not know are skipped: they cannot run, so
/// promising "until Learning time at 09:00" for one would be a lie. A "start"
/// is a change INTO a runnable label (Free 16:00–17:00 then Learning
/// 17:00–18:00 starts a span at 17:00), so this also answers "what follows" for
/// a warning while open.
pub fn next_window(
    sched: &GrantSchedule,
    known: &dyn Fn(&str) -> bool,
    now_unix: i64,
) -> Option<NextWindow> {
    if !sched.is_supported_version() || sched.paused == Some(true) || is_unbounded(sched) {
        return None;
    }
    let (tz, now) = local_now(sched, now_unix)?;
    let minute_now = now.hour() * 60 + now.minute();
    let today = now.date_naive();
    for day_offset in 0..=1u8 {
        let probe = today.checked_add_days(chrono::Days::new(u64::from(day_offset)))?;
        let wins = day_windows(
            sched,
            &probe.format("%Y-%m-%d").to_string(),
            probe.weekday().num_days_from_sunday(),
        );
        for b in boundaries(&wins) {
            if day_offset == 0 && b <= minute_now {
                continue;
            }
            let label = label_at(&wins, known, b);
            let starts = b == 0 || label_at(&wins, known, b - 1) != label;
            if label.runnable() && starts {
                let start = local_wall_ts(tz, probe, b);
                if start > now_unix {
                    return Some(NextWindow {
                        time: match label {
                            Label::Time(id) => Some(id.to_string()),
                            _ => None,
                        },
                        start,
                        start_hhmm: hhmm(b),
                        day_offset,
                    });
                }
            }
        }
    }
    None
}

/// The ward's default "ask for more" type **when it is not inside a window**:
/// the time whose window **most recently ended today**, else the **next one up
/// today**, else Free. `None` means Free time.
///
/// This is ONLY the not-in-window half of the design's rule. The full rule
/// applies "inside a window that cannot run (budget spent) => that window's
/// time" FIRST, from [`evaluate_times`]; a caller must not use this function
/// alone as the default while a window is open. Only spans that can run count (an unknown tag is not
/// a time the child could be given more of).
///
/// A span still in progress has not "ended", so called inside a Free window
/// that follows a finished Learning window this returns Learning: another
/// reason the in-window rule must be applied first.
pub fn default_ask_time(
    sched: &GrantSchedule,
    known: &dyn Fn(&str) -> bool,
    now_unix: i64,
) -> Option<String> {
    if !sched.is_supported_version() || sched.paused == Some(true) || is_unbounded(sched) {
        return None;
    }
    let (_tz, now) = local_now(sched, now_unix)?;
    let minute_now = now.hour() * 60 + now.minute();
    let wins = day_windows(
        sched,
        &now.format("%Y-%m-%d").to_string(),
        now.weekday().num_days_from_sunday(),
    );

    // Today's maximal runs of one runnable label, as (start, end, time).
    let mut spans: Vec<(u32, u32, Option<&str>)> = Vec::new();
    for b in boundaries(&wins) {
        // A boundary inside the span just recorded is not a new span.
        if spans.last().is_some_and(|&(_, e, _)| b < e) {
            continue;
        }
        let label = label_at(&wins, known, b);
        if !label.runnable() {
            continue;
        }
        let end = span_end_minute(&wins, known, b, &label);
        let time = match label {
            Label::Time(id) => Some(id),
            _ => None,
        };
        spans.push((b, end, time));
    }

    if let Some(&(_, _, t)) = spans
        .iter()
        .filter(|&&(_, e, _)| e <= minute_now)
        .max_by_key(|&&(_, e, _)| e)
    {
        return t.map(str::to_string);
    }
    spans
        .iter()
        .filter(|&&(s, _, _)| s > minute_now)
        .min_by_key(|&&(s, _, _)| s)
        .and_then(|&(_, _, t)| t.map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::WeeklySchedule;

    // Mon 2026-06-29 00:00:00 UTC.
    const MON: i64 = 1_782_691_200;

    fn at(h: i64, m: i64) -> i64 {
        MON + h * 3600 + m * 60
    }

    fn w(start: &str, end: &str, time: Option<&str>) -> GrantScheduleWindow {
        GrantScheduleWindow {
            start: start.into(),
            end: end.into(),
            time: time.map(str::to_string),
        }
    }

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
            issued_at: 0,
        }
    }

    fn only_learning(id: &str) -> bool {
        id == "learning"
    }

    fn nothing_known(_: &str) -> bool {
        false
    }

    fn open(time: Option<&str>, end: i64) -> TimedStatus {
        TimedStatus::Open {
            time: time.map(str::to_string),
            span_end: end,
        }
    }

    #[test]
    fn an_untagged_schedule_is_free_and_matches_the_classic_close() {
        let s = sched(vec![w("16:00", "19:00", None)]);
        assert_eq!(
            evaluate_times(&s, &only_learning, at(16, 30)),
            open(None, at(19, 0))
        );
        assert!(matches!(
            evaluate_times(&s, &only_learning, at(15, 0)),
            TimedStatus::Locked {
                cause: LockCause::OutsideWindows,
                seconds_to_open: Some(3600)
            }
        ));
    }

    #[test]
    fn a_tagged_window_carves_free_and_span_end_is_the_carve_edge() {
        let s = sched(vec![
            w("16:00", "19:00", None),
            w("17:00", "18:00", Some("learning")),
        ]);
        let k = &only_learning;
        assert_eq!(evaluate_times(&s, k, at(16, 30)), open(None, at(17, 0)));
        assert_eq!(
            evaluate_times(&s, k, at(17, 0)),
            open(Some("learning"), at(18, 0))
        );
        assert_eq!(evaluate_times(&s, k, at(18, 0)), open(None, at(19, 0)));
    }

    #[test]
    fn contiguous_same_time_windows_form_one_span() {
        let s = sched(vec![w("09:00", "10:00", None), w("10:00", "11:00", None)]);
        assert_eq!(
            evaluate_times(&s, &nothing_known, at(9, 30)),
            open(None, at(11, 0))
        );
    }

    #[test]
    fn an_unknown_tag_locks_its_span_even_over_a_free_window() {
        let s = sched(vec![
            w("16:00", "19:00", None),
            w("17:00", "18:00", Some("learning")),
        ]);
        // No times clause at all: the carve locks, it does not open as Free.
        match evaluate_times(&s, &nothing_known, at(17, 30)) {
            TimedStatus::Locked {
                cause: LockCause::UnknownTime(t),
                seconds_to_open,
            } => {
                assert_eq!(t, "learning");
                assert_eq!(seconds_to_open, Some(30 * 60), "Free resumes at 18:00");
            }
            other => panic!("{other:?}"),
        }
        // Untagged spans around it are unaffected.
        assert_eq!(
            evaluate_times(&s, &nothing_known, at(16, 30)),
            open(None, at(17, 0))
        );
    }

    #[test]
    fn overlapping_tagged_windows_resolve_to_the_earlier_start_then_array_order() {
        let known = |id: &str| id == "a" || id == "b";
        let s = sched(vec![
            w("10:00", "12:00", Some("b")),
            w("09:00", "11:00", Some("a")),
        ]);
        assert_eq!(
            evaluate_times(&s, &known, at(10, 30)),
            open(Some("a"), at(11, 0))
        );
        assert_eq!(
            evaluate_times(&s, &known, at(11, 30)),
            open(Some("b"), at(12, 0))
        );
        // Same start: array order.
        let s = sched(vec![
            w("09:00", "10:00", Some("b")),
            w("09:00", "11:00", Some("a")),
        ]);
        assert_eq!(
            evaluate_times(&s, &known, at(9, 30)),
            open(Some("b"), at(10, 0))
        );
        assert_eq!(
            evaluate_times(&s, &known, at(10, 30)),
            open(Some("a"), at(11, 0))
        );
    }

    #[test]
    fn paused_empty_and_unreadable_schedules_keep_their_classic_meaning() {
        let mut s = sched(vec![w("09:00", "10:00", None)]);
        s.paused = Some(true);
        assert!(matches!(
            evaluate_times(&s, &nothing_known, at(9, 30)),
            TimedStatus::Locked {
                cause: LockCause::Paused,
                seconds_to_open: None
            }
        ));
        let mut empty = sched(vec![]);
        empty.weekly = WeeklySchedule::default();
        assert_eq!(
            evaluate_times(&empty, &nothing_known, at(9, 30)),
            TimedStatus::Unbounded
        );
        let mut bad = sched(vec![w("09:00", "10:00", None)]);
        bad.tz = "Not/AZone".into();
        assert_eq!(
            evaluate_times(&bad, &nothing_known, at(9, 30)),
            TimedStatus::Unparseable
        );
        let mut future = sched(vec![w("09:00", "10:00", None)]);
        future.v = 2;
        assert_eq!(
            evaluate_times(&future, &nothing_known, at(9, 30)),
            TimedStatus::Unparseable
        );
    }

    #[test]
    fn next_window_names_what_starts_next_and_skips_unknown_tags() {
        let s = sched(vec![
            w("09:00", "12:00", Some("learning")),
            w("16:00", "19:00", None),
            w("20:00", "21:00", Some("mystery")),
        ]);
        let k = &only_learning;
        let n = next_window(&s, k, at(7, 0)).unwrap();
        assert_eq!(n.time.as_deref(), Some("learning"));
        assert_eq!(
            (n.start, n.start_hhmm.as_str(), n.day_offset),
            (at(9, 0), "09:00", 0)
        );
        let n = next_window(&s, k, at(13, 0)).unwrap();
        assert_eq!((n.time, n.start_hhmm.as_str()), (None, "16:00"));
        // After Free closes, only the unknown tag remains today; tomorrow
        // (Tuesday) has no windows: "Locked today".
        assert_eq!(next_window(&s, k, at(19, 30)), None);
        // While open, "what follows" is the next span start.
        let n = next_window(&s, k, at(10, 0)).unwrap();
        assert_eq!(n.start_hhmm, "16:00");
    }

    #[test]
    fn default_ask_time_is_most_recently_ended_then_next_up_then_free() {
        let s = sched(vec![
            w("09:00", "12:00", Some("learning")),
            w("16:00", "19:00", None),
        ]);
        let k = &only_learning;
        // Before anything today: the next one up.
        assert_eq!(
            default_ask_time(&s, k, at(7, 0)).as_deref(),
            Some("learning")
        );
        // After Learning ended, before Free: Learning is the most recent.
        assert_eq!(
            default_ask_time(&s, k, at(13, 0)).as_deref(),
            Some("learning")
        );
        // After Free ended: Free (None) is the most recent.
        assert_eq!(default_ask_time(&s, k, at(20, 0)), None);
        // No windows today at all: Free.
        let tue = at(24 + 10, 0);
        assert_eq!(default_ask_time(&s, k, tue), None);
    }

    /// A day with a missing hour: 00:00–06:00 London on the spring-forward date
    /// ends at 05:00 UTC, five real hours after midnight, not six.
    #[test]
    fn span_end_counts_a_short_dst_day_as_it_elapses() {
        let mut s = sched(vec![]);
        s.tz = "Europe/London".into();
        s.weekly = WeeklySchedule {
            sun: Some(vec![w("00:00", "06:00", None)]),
            ..Default::default()
        };
        let midnight = 1774742400; // 2026-03-29T00:00:00Z == 00:00 GMT
        assert_eq!(
            evaluate_times(&s, &nothing_known, midnight + 1800),
            open(None, midnight + 5 * 3600)
        );
    }
}
