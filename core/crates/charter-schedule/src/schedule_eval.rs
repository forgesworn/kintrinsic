//! Schedule evaluation. `evaluate_schedule` is an **exact** port of the SDK's
//! `evaluateSchedule` (same order, start-inclusive/end-exclusive, the
//! midnight-crossing same-day quirk, and fail-OPEN on a malformed tz) used ONLY
//! for parity. `evaluate_grant_schedule` is the runtime-canonical evaluator the
//! enforcer uses (and is wrapped fail-SAFE at the enforcement layer).

use chrono::{DateTime, Datelike, Timelike};
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
                // Seconds until close (today).
                let close_minute = *end;
                let secs = (close_minute - minute) as u64 * 60 - now.second() as u64;
                return ScheduleStatus::Open {
                    seconds_to_close: secs,
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
    let now_minute = now.hour() * 60 + now.minute();
    for day_offset in 0..8i64 {
        let probe = now + chrono::Duration::days(day_offset);
        let day = probe.weekday().num_days_from_sunday();
        let date_key = probe.format("%Y-%m-%d").to_string();
        if let Some(windows) = windows_for(sched, &date_key, day) {
            let mut starts: Vec<u32> = windows.iter().map(|(s, _)| *s).collect();
            starts.sort_unstable();
            for start in starts {
                if day_offset > 0 || start > now_minute {
                    // Distance from NOW: whole days ahead + the window's start,
                    // minus the minutes already elapsed today. (Zeroing the
                    // elapsed term for future days overcounted by the current
                    // time of day — a Sat-19:19 lock said "back Mon 02:18"
                    // instead of "tomorrow 07:00".) Safe unsigned: day_offset
                    // >= 1 makes the sum >= 1440 > now_minute (<= 1439).
                    let minutes_ahead =
                        (day_offset as u64) * 1440 + (start as u64) - (now_minute as u64);
                    // Subtract current seconds-into-minute for precision.
                    let secs = minutes_ahead * 60 - now.second() as u64;
                    return Some(secs);
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
