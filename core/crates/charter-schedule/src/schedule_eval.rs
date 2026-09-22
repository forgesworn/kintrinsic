//! Schedule evaluation. `evaluate_schedule` is an **exact** port of the SDK's
//! `evaluateSchedule` (same order, start-inclusive/end-exclusive, the
//! midnight-crossing same-day quirk, and fail-OPEN on a malformed tz) used ONLY
//! for parity. `evaluate_grant_schedule` is the runtime-canonical evaluator the
//! enforcer uses (and is wrapped fail-SAFE at the enforcement layer).

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::clause::{ChartedClause, GrantSchedule};

/// The SDK's evaluation reasons (snake_case wire strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluateReason {
    ClauseRevoked,
    ClauseExpired,
    AlwaysAllow,
    NoClause,
    ScheduleLocked,
}

/// The SDK's evaluation result (`{allow, reason?}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluateResult {
    pub allow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<EvaluateReason>,
}

fn day_minute_in_tz(now_unix: i64, tz: Tz) -> (u32, u32) {
    let dt: DateTime<Tz> = DateTime::from_timestamp(now_unix, 0)
        .expect("valid timestamp")
        .with_timezone(&tz);
    let day = dt.weekday().num_days_from_sunday(); // Sun=0..Sat=6
    let minute = dt.hour() * 60 + dt.minute();
    (day, minute)
}

/// EXACT port of the SDK `evaluateSchedule` (parity shim — fail-OPEN on bad tz).
pub fn evaluate_schedule(clause: &ChartedClause, now_unix: i64) -> EvaluateResult {
    if clause.revoked {
        return EvaluateResult {
            allow: false,
            reason: Some(EvaluateReason::ClauseRevoked),
        };
    }
    if let Some(end) = clause.end_date {
        if now_unix > end {
            return EvaluateResult {
                allow: false,
                reason: Some(EvaluateReason::ClauseExpired),
            };
        }
    }
    if clause.windows.is_empty() {
        return EvaluateResult {
            allow: true,
            reason: Some(EvaluateReason::AlwaysAllow),
        };
    }
    let tz: Tz = match clause.timezone.parse() {
        Ok(tz) => tz,
        Err(_) => {
            return EvaluateResult {
                allow: true,
                reason: Some(EvaluateReason::NoClause),
            }
        }
    };
    let (day, minute) = day_minute_in_tz(now_unix, tz);
    for w in &clause.windows {
        if !w.days_of_week.contains(&(day as u8)) {
            continue;
        }
        let allow = if w.end_minute > w.start_minute {
            minute >= w.start_minute && minute < w.end_minute
        } else {
            // Crosses midnight (same-day quirk): early-morning OR late-night.
            minute >= w.start_minute || minute < w.end_minute
        };
        if allow {
            return EvaluateResult {
                allow: true,
                reason: None,
            };
        }
    }
    EvaluateResult {
        allow: false,
        reason: Some(EvaluateReason::ScheduleLocked),
    }
}

/// Runtime-canonical schedule status with the countdown the enforcer needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleStatus {
    /// No schedule constraint at all (empty schedule).
    Unbounded,
    /// Within an allowed window; seconds until it closes.
    Open { seconds_to_close: u64 },
    /// Locked; seconds until the next window opens (None if none within 8 days).
    Locked { seconds_to_open: Option<u64> },
    /// The clause could not be parsed (bad tz). The enforcement layer treats
    /// this fail-SAFE (lock / last good clause) — never fail-open.
    Unparseable,
}

/// The unix instant of `minute_of_day` (minutes past local midnight) on `date`
/// in `tz`, robust across DST — the same ambiguous/gap discipline
/// `enforcer::local_midnight_ts` uses, which is why that function now delegates
/// here: an ambiguous (fall-back) wall time takes the EARLIER instant, and one
/// skipped by a spring-forward gap takes the first valid instant after the gap.
/// Never silently falls back to "now".
///
/// Every schedule countdown is a difference of two of these, never arithmetic
/// on minute-of-day: a day containing a transition is 1380 or 1500 minutes
/// long, so `(end − now) * 60` is wrong by exactly an hour across every
/// spring-forward / fall-back boundary — and that number is what the lock
/// screen shows as "access resumes in …".
pub(crate) fn local_wall_ts(tz: Tz, date: NaiveDate, minute_of_day: u32) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("valid midnight")
        + Duration::minutes(i64::from(minute_of_day));
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.timestamp(),
        LocalResult::Ambiguous(earlier, _later) => earlier.timestamp(),
        LocalResult::None => (1..=180)
            .find_map(
                |m| match tz.from_local_datetime(&(naive + Duration::minutes(m))) {
                    LocalResult::Single(dt) => Some(dt.timestamp()),
                    LocalResult::Ambiguous(e, _) => Some(e.timestamp()),
                    LocalResult::None => None,
                },
            )
            .unwrap_or_else(|| Utc.from_utc_datetime(&naive).timestamp()),
    }
}

fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

fn windows_for(sched: &GrantSchedule, date_key: &str, day: u32) -> Option<Vec<(u32, u32)>> {
    // An override for this date REPLACES the weekly entry (empty array = blocked).
    if let Some(ovr) = sched.overrides.as_ref().and_then(|o| o.get(date_key)) {
        return Some(parse_windows(ovr));
    }
    sched.weekly.for_day(day).map(|w| parse_windows(w))
}

fn parse_windows(windows: &[crate::clause::GrantScheduleWindow]) -> Vec<(u32, u32)> {
    windows
        .iter()
        .filter_map(|w| Some((parse_hhmm(&w.start)?, parse_hhmm(&w.end)?)))
        .filter(|(s, e)| s < e)
        .collect()
}

/// Runtime-canonical schedule evaluation (weekly + per-date overrides + paused,
/// `"HH:MM"`, tz-aware). Returns the open/locked status + countdown.
pub fn evaluate_grant_schedule(sched: &GrantSchedule, now_unix: i64) -> ScheduleStatus {
    // A body version this build does not implement reads as UNPARSEABLE — the
    // same fail-SAFE the enforcement layer already gives a clause it cannot
    // read, and the only honest answer when serde has silently dropped fields
    // whose meaning we do not know. Checked before `paused`, so no future
    // field can reach a v1 arm.
    if !sched.is_supported_version() {
        return ScheduleStatus::Unparseable;
    }
    if sched.paused == Some(true) {
        return ScheduleStatus::Locked {
            seconds_to_open: None,
        };
    }
    let tz: Tz = match sched.tz.parse() {
        Ok(tz) => tz,
        Err(_) => return ScheduleStatus::Unparseable,
    };
    let has_overrides = sched
        .overrides
        .as_ref()
        .map(|o| !o.is_empty())
        .unwrap_or(false);
    if sched.weekly.is_empty() && !has_overrides {
        return ScheduleStatus::Unbounded;
    }

    let now: DateTime<Tz> = DateTime::from_timestamp(now_unix, 0)
        .expect("valid timestamp")
        .with_timezone(&tz);
    let day = now.weekday().num_days_from_sunday();
    let minute = now.hour() * 60 + now.minute();
    let date_key = now.format("%Y-%m-%d").to_string();

    // Current day: are we inside a window?
    if let Some(windows) = windows_for(sched, &date_key, day) {
        for (start, end) in &windows {
            if minute >= *start && minute < *end {
                // Seconds until close: the DIFFERENCE OF TWO INSTANTS, so a
                // 23- or 25-hour day is counted as it actually elapses. A
                // 00:00–06:00 window on the spring-forward date used to report
                // six hours of wall clock where only five will pass, which put
                // this countdown an hour out of step with the enforcer's own
                // end-of-day cap. Saturated at zero because the fall-back
                // hour's EARLIER instant can already be behind us on the
                // second pass — closing early is the fail-safe direction.
                let close = local_wall_ts(tz, now.date_naive(), *end);
                return ScheduleStatus::Open {
                    seconds_to_close: (close - now_unix).max(0) as u64,
                };
            }
        }
    }

    // Locked: scan forward (today's later windows, then up to 8 days) for the
    // next open.
    ScheduleStatus::Locked {
        seconds_to_open: next_open_secs(sched, &tz, now_unix),
    }
}

/// Whether a single app may run right now, per its [`AppRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAccess {
    /// The app may open now.
    Allowed,
    /// The app must not open — blocked outright, or outside its allowed hours.
    Blocked,
}

/// Evaluate one per-app rule at `now_unix`. The `blocked` flag wins outright;
/// otherwise, if the rule carries an allowed-hours schedule, the app is Allowed
/// only inside a window (Open/Unbounded) and Blocked when Locked or the schedule
/// won't parse — fail-SAFE, never fail-open, matching the device schedule. With
/// no per-app schedule the rule imposes no time constraint (Allowed).
pub fn evaluate_app_rule(rule: &crate::clause::AppRule, now_unix: i64) -> AppAccess {
    if rule.blocked {
        return AppAccess::Blocked;
    }
    match &rule.schedule {
        None => AppAccess::Allowed,
        Some(sched) => match evaluate_grant_schedule(sched, now_unix) {
            ScheduleStatus::Unbounded | ScheduleStatus::Open { .. } => AppAccess::Allowed,
            ScheduleStatus::Locked { .. } | ScheduleStatus::Unparseable => AppAccess::Blocked,
        },
    }
}

/// Evaluate every rule in a [`GrantAppRules`] set at `now_unix`, returning each
/// app's `pkg` paired with its access decision — the per-app enforcement input.
pub fn evaluate_app_rules(
    rules: &crate::clause::GrantAppRules,
    now_unix: i64,
) -> Vec<(String, AppAccess)> {
    rules
        .rules
        .iter()
        .map(|r| (r.pkg.clone(), evaluate_app_rule(r, now_unix)))
        .collect()
}

fn next_open_secs(sched: &GrantSchedule, tz: &Tz, now_unix: i64) -> Option<u64> {
    let now: DateTime<Tz> = DateTime::from_timestamp(now_unix, 0)?.with_timezone(tz);
    let today = now.date_naive();
    for day_offset in 0..8u64 {
        // CALENDAR days, not `now + 86400 * n`: a fixed 24-hour step lands on
        // the wrong local date across a DST transition (at 00:30 on a 25-hour
        // day it lands at 23:30 the SAME day, skipping a day entirely).
        let probe = today.checked_add_days(chrono::Days::new(day_offset))?;
        let day = probe.weekday().num_days_from_sunday();
        let date_key = probe.format("%Y-%m-%d").to_string();
        if let Some(windows) = windows_for(sched, &date_key, day) {
            let mut starts: Vec<u32> = windows.iter().map(|(s, _)| *s).collect();
            starts.sort_unstable();
            for start in starts {
                // The countdown is the distance between two INSTANTS, so a
                // short or long day counts as it will actually elapse.
                // `day_offset * 1440 + start - now_minute` assumed every day
                // was 1440 minutes and so was an hour out across every DST
                // boundary — on the very number the lock screen shows as
                // "access resumes in …".
                let open = local_wall_ts(*tz, probe, start);
                if open > now_unix {
                    return Some((open - now_unix) as u64);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    fn every_day_window(start: &str, end: &str) -> GrantSchedule {
        let win = || {
            Some(vec![crate::clause::GrantScheduleWindow {
                start: start.into(),
                end: end.into(),
            }])
        };
        GrantSchedule {
            v: 1,
            tz: "Etc/UTC".into(),
            paused: None,
            weekly: crate::clause::WeeklySchedule {
                mon: win(),
                tue: win(),
                wed: win(),
                thu: win(),
                fri: win(),
                sat: win(),
                sun: win(),
            },
            overrides: None,
            issued_at: 0,
        }
    }

    #[test]
    fn app_rule_blocked_flag_wins_over_schedule() {
        // Even inside an allowed window, a blocked app is Blocked.
        let rule = crate::clause::AppRule {
            pkg: "com.example.game".into(),
            label: None,
            blocked: true,
            schedule: Some(every_day_window("00:00", "23:59")),
        };
        assert_eq!(evaluate_app_rule(&rule, 1783166400), AppAccess::Blocked);
    }

    #[test]
    fn app_rule_no_schedule_is_allowed() {
        let rule = crate::clause::AppRule {
            pkg: "com.example.game".into(),
            label: None,
            blocked: false,
            schedule: None,
        };
        assert_eq!(evaluate_app_rule(&rule, 1783166400), AppAccess::Allowed);
    }

    #[test]
    fn app_rule_schedule_gates_by_window() {
        // 2026-07-04 (Sat) UTC; window every day 07:00–08:00.
        let rule = crate::clause::AppRule {
            pkg: "com.example.game".into(),
            label: Some("Game".into()),
            blocked: false,
            schedule: Some(every_day_window("07:00", "08:00")),
        };
        let sat_noon = 1783166400; // 12:00 UTC — outside the window
        let sat_0730 = 1783166400 - 4 * 3600 - 30 * 60; // 07:30 UTC — inside
        assert_eq!(evaluate_app_rule(&rule, sat_noon), AppAccess::Blocked);
        assert_eq!(evaluate_app_rule(&rule, sat_0730), AppAccess::Allowed);
    }

    #[test]
    fn app_rule_unparseable_schedule_fails_safe_blocked() {
        let mut sched = every_day_window("07:00", "08:00");
        sched.tz = "Not/AZone".into();
        let rule = crate::clause::AppRule {
            pkg: "com.example.game".into(),
            label: None,
            blocked: false,
            schedule: Some(sched),
        };
        assert_eq!(evaluate_app_rule(&rule, 1783166400), AppAccess::Blocked);
    }

    #[test]
    fn evaluate_app_rules_maps_each_pkg() {
        let rules = crate::clause::GrantAppRules {
            v: 1,
            issued_at: 0,
            rules: vec![
                crate::clause::AppRule {
                    pkg: "a".into(),
                    label: None,
                    blocked: true,
                    schedule: None,
                },
                crate::clause::AppRule {
                    pkg: "b".into(),
                    label: None,
                    blocked: false,
                    schedule: None,
                },
            ],
        };
        let out = evaluate_app_rules(&rules, 1783166400);
        assert_eq!(
            out,
            vec![
                ("a".to_string(), AppAccess::Blocked),
                ("b".to_string(), AppAccess::Allowed),
            ]
        );
    }

    #[test]
    fn next_open_measures_from_now_not_midnight() {
        // Every day 07:00–08:00 (the golden vectors never cover a NEXT-DAY
        // open, which let a from-midnight overcount ship: a Sat-19:19 lock
        // claimed "back Mon 02:18" instead of Sun 07:00).
        let win = || {
            Some(vec![crate::clause::GrantScheduleWindow {
                start: "07:00".into(),
                end: "08:00".into(),
            }])
        };
        let sched = GrantSchedule {
            v: 1,
            tz: "Etc/UTC".into(),
            paused: None,
            weekly: crate::clause::WeeklySchedule {
                mon: win(),
                tue: win(),
                wed: win(),
                thu: win(),
                fri: win(),
                sat: win(),
                sun: win(),
            },
            overrides: None,
            issued_at: 0,
        };
        // 2026-07-04 (Sat) 19:19:00 UTC -> next open Sun 07:00, in 11h41m.
        let sat_1919 = 1783166400 + 7 * 3600 + 19 * 60; // 12:00 UTC + 7h19m
        let status = evaluate_grant_schedule(&sched, sat_1919);
        let ScheduleStatus::Locked { seconds_to_open } = status else {
            panic!("expected locked, got {status:?}");
        };
        assert_eq!(seconds_to_open, Some((11 * 60 + 41) * 60));
        // Same-day later window still measures from now: 06:00 -> 07:00 = 1h.
        let sat_0600 = 1783166400 - 6 * 3600; // 06:00 UTC
        let ScheduleStatus::Locked { seconds_to_open } = evaluate_grant_schedule(&sched, sat_0600)
        else {
            panic!("expected locked");
        };
        assert_eq!(seconds_to_open, Some(3600));
    }

    fn every_day_window_in(tz: &str, start: &str, end: &str) -> GrantSchedule {
        let mut s = every_day_window(start, end);
        s.tz = tz.into();
        s
    }

    /// B5 (spring forward). Europe/London 2027-03-28: 01:00 GMT becomes 02:00
    /// BST, so the local day is 1380 minutes long. Minute-of-day arithmetic
    /// said six hours of window where only five will pass, and eight hours to
    /// the next open where only seven will — an hour wrong on the number the
    /// lock screen shows as "access resumes in …", and an hour out of step
    /// with the enforcer's own (DST-correct) end-of-day cap.
    #[test]
    fn countdowns_survive_a_spring_forward() {
        let sched = every_day_window_in("Europe/London", "00:00", "06:00");
        // Sun 2027-03-28 00:30 local: 06:00 local is 4h30m away, not 5h30m.
        let at_0030 = 1_806_193_800;
        assert_eq!(
            evaluate_grant_schedule(&sched, at_0030),
            ScheduleStatus::Open {
                seconds_to_close: 4 * 3600 + 1800
            }
        );

        // Sat 2027-03-27 23:00 local, daily 07:00–08:00: the next open is 7h
        // away (23:00 GMT → 07:00 BST), not the 8h a 1440-minute day implies.
        let morning = every_day_window_in("Europe/London", "07:00", "08:00");
        let sat_2300 = 1_806_188_400;
        assert_eq!(
            evaluate_grant_schedule(&morning, sat_2300),
            ScheduleStatus::Locked {
                seconds_to_open: Some(7 * 3600)
            }
        );
    }

    /// B5 (fall back). Europe/London 2026-10-25: 02:00 BST becomes 01:00 GMT,
    /// a 1500-minute day. The error runs the other way — the countdowns were
    /// an hour SHORT.
    #[test]
    fn countdowns_survive_a_fall_back() {
        let sched = every_day_window_in("Europe/London", "00:00", "06:00");
        // Sun 2026-10-25 00:30 local (BST): 06:00 local (GMT) is 6h30m away.
        let at_0030 = 1_792_884_600;
        assert_eq!(
            evaluate_grant_schedule(&sched, at_0030),
            ScheduleStatus::Open {
                seconds_to_close: 6 * 3600 + 1800
            }
        );

        // Sat 2026-10-24 23:00 local, daily 07:00–08:00: 9h to the next open.
        let morning = every_day_window_in("Europe/London", "07:00", "08:00");
        let sat_2300 = 1_792_879_200;
        assert_eq!(
            evaluate_grant_schedule(&morning, sat_2300),
            ScheduleStatus::Locked {
                seconds_to_open: Some(9 * 3600)
            }
        );
    }

    /// The day-walk steps CALENDAR days. A fixed 24-hour step lands on the
    /// wrong local date across a transition — at 00:30 on a 25-hour day it
    /// lands at 23:30 the SAME day — which would probe one date twice and skip
    /// another entirely.
    #[test]
    fn the_next_open_walk_never_skips_a_calendar_day() {
        // Only SUNDAY has a window; asked on Sun 2026-10-25 00:30 (the 25-hour
        // day), the answer must be 07:00 the same morning — 7h30m of real
        // time away, because the hour 01:00–01:59 is lived twice.
        let mut sched = every_day_window_in("Europe/London", "07:00", "08:00");
        sched.weekly.mon = None;
        sched.weekly.tue = None;
        sched.weekly.wed = None;
        sched.weekly.thu = None;
        sched.weekly.fri = None;
        sched.weekly.sat = None;
        assert_eq!(
            evaluate_grant_schedule(&sched, 1_792_884_600),
            ScheduleStatus::Locked {
                seconds_to_open: Some(7 * 3600 + 1800)
            }
        );
        // …and from Sunday evening the walk carries on to the NEXT Sunday
        // rather than answering the same morning twice.
        let sun_1900 = 1_792_884_600 + 19 * 3600 + 30 * 60; // Sun 19:00 GMT
        let ScheduleStatus::Locked { seconds_to_open } = evaluate_grant_schedule(&sched, sun_1900)
        else {
            panic!("expected locked");
        };
        assert_eq!(seconds_to_open, Some(6 * 86_400 + 12 * 3600));
    }

    /// G1: a body version this build does not implement is UNPARSEABLE — the
    /// fail-SAFE the enforcer turns into a `Malformed` lock — never a v1 body
    /// whose unknown fields serde silently dropped.
    #[test]
    fn a_future_body_version_is_unparseable() {
        let mut sched = every_day_window("00:00", "23:59");
        sched.v = crate::clause::SCHEDULE_VERSION + 1;
        assert_eq!(
            evaluate_grant_schedule(&sched, 1_783_166_400),
            ScheduleStatus::Unparseable
        );
        // An empty weekly set would otherwise be Unbounded — the version check
        // outranks every arm, including the one that imposes nothing.
        let empty = GrantSchedule {
            v: crate::clause::SCHEDULE_VERSION + 1,
            tz: "Etc/UTC".into(),
            paused: None,
            weekly: crate::clause::WeeklySchedule::default(),
            overrides: None,
            issued_at: 0,
        };
        assert_eq!(
            evaluate_grant_schedule(&empty, 1_783_166_400),
            ScheduleStatus::Unparseable
        );
        // A per-app rule carrying one is Blocked, same fail-safe direction.
        let rule = crate::clause::AppRule {
            pkg: "com.example.game".into(),
            label: None,
            blocked: false,
            schedule: Some(sched),
        };
        assert_eq!(evaluate_app_rule(&rule, 1_783_166_400), AppAccess::Blocked);
    }

    #[derive(serde::Deserialize)]
    struct Vectors {
        vectors: Vec<Vector>,
    }
    #[derive(serde::Deserialize)]
    struct Vector {
        name: String,
        clause: ChartedClause,
        now_unix: i64,
        expect: EvaluateResult,
    }

    #[test]
    fn schedule_parity_matches_ts_sdk() {
        let v: Vectors = charter_testkit::golden::load_json("schedule/schedule_vectors.json");
        assert!(!v.vectors.is_empty());
        for case in v.vectors {
            let got = evaluate_schedule(&case.clause, case.now_unix);
            assert_eq!(got, case.expect, "parity mismatch for vector {}", case.name);
        }
    }
}
