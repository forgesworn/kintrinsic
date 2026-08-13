//! Child-facing schedule/usage lines for the lock panel — the answer to "when
//! can I come back on?". Composed here (the daemon holds the policy + ledger),
//! rendered verbatim by `charter-lock`. Pure string-building: fully unit-tested.

use charter_schedule::{
    enforcement_tz_of, GrantBudget, GrantSchedule, GrantScheduleWindow, WeeklySchedule,
};
use chrono::{Datelike, TimeZone, Timelike};

/// Everything extra the lock panel shows for one locked child:
/// `detail` replaces the static reason line when we can say something better
/// ("You can come back at 07:00 tomorrow."); `lines` render beneath it.
///
/// `title` is the same idea one level up — set ONLY when the static per-reason
/// headline would be wrong rather than merely vague. Today that is the dormant
/// device: it locks as `LockReason::Schedule` (it must — the grant routing keys
/// off that reason) but "Outside allowed hours" describes a device that has
/// allowed hours, and this one has none.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LockInfo {
    pub title: Option<String>,
    pub detail: Option<String>,
    pub lines: Vec<String>,
}

/// Compose the panel's schedule/usage block.
pub fn lock_info(
    schedule: Option<&GrantSchedule>,
    budget: Option<&GrantBudget>,
    next_open_secs: Option<i64>,
    used_today_secs: u64,
    now: i64,
) -> LockInfo {
    let tz = enforcement_tz_of(schedule, budget);
    let mut info = LockInfo::default();

    if let Some(secs) = next_open_secs {
        if secs > 0 {
            info.detail = Some(back_at_line(tz, now, secs));
        }
    }

    // A dormant device (wire-paused: off until a guardian opens it) has nothing
    // to list. Seven lines of "no screen time" restate the title at length and
    // read like a schedule the guardian forgot to fill in, which is the very
    // impression the posture exists to correct. It gets the honest headline and
    // reason instead, both straight from the shared child-facing copy.
    if let Some(s) = schedule {
        if crate::enforcer_runtime::schedule_is_dormant(Some(s)) {
            let (title, detail) = crate::enforcer_runtime::lock_message_for(
                charter_schedule::LockReason::Schedule,
                true,
            );
            info.title = Some(title.to_string());
            info.detail = Some(detail.to_string());
        } else {
            info.lines.extend(week_lines(&s.weekly));
        }
    }
    if let Some(b) = budget {
        if let Some(daily) = b.daily_minutes {
            let mut line = format!("Screen time: up to {} a day", fmt_mins(daily as u64));
            if used_today_secs >= 60 {
                line.push_str(&format!(" — {} used today", fmt_mins(used_today_secs / 60)));
            }
            info.lines.push(line);
        }
    }
    info
}

/// "You can come back at 07:00 tomorrow." (today / tomorrow / weekday, in the
/// child's tz).
fn back_at_line(tz: chrono_tz::Tz, now: i64, next_open_secs: i64) -> String {
    let open = tz.timestamp_opt(now + next_open_secs, 0).single();
    let today = tz.timestamp_opt(now, 0).single();
    let (Some(open), Some(today)) = (open, today) else {
        return "You can come back when your next window opens.".to_string();
    };
    let hm = format!("{:02}:{:02}", open.hour(), open.minute());
    let days_ahead = open.date_naive().num_days_from_ce() - today.date_naive().num_days_from_ce();
    match days_ahead {
        0 => format!("You can come back at {hm} today."),
        1 => format!("You can come back at {hm} tomorrow."),
        _ => format!("You can come back on {} at {hm}.", open.weekday()),
    }
}

/// The week's allowed hours, Mon→Sun, with identical consecutive days grouped:
/// "Mon–Fri   07:00 – 20:00". A day with no windows reads "no screen time".
fn week_lines(weekly: &WeeklySchedule) -> Vec<String> {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let per_day: [String; 7] = [
        windows_text(weekly.mon.as_deref()),
        windows_text(weekly.tue.as_deref()),
        windows_text(weekly.wed.as_deref()),
        windows_text(weekly.thu.as_deref()),
        windows_text(weekly.fri.as_deref()),
        windows_text(weekly.sat.as_deref()),
        windows_text(weekly.sun.as_deref()),
    ];
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..=7 {
        if i == 7 || per_day[i] != per_day[start] {
            let span = if i - start == 1 {
                DAYS[start].to_string()
            } else {
                format!("{}–{}", DAYS[start], DAYS[i - 1])
            };
            out.push(format!("{span}   {}", per_day[start]));
            start = i;
        }
    }
    out
}

fn windows_text(windows: Option<&[GrantScheduleWindow]>) -> String {
    match windows {
        Some(ws) if !ws.is_empty() => ws
            .iter()
            .map(|w| format!("{} – {}", w.start, w.end))
            .collect::<Vec<_>>()
            .join(", "),
        _ => "no screen time".to_string(),
    }
}

/// "2h", "1h 05m", "45m".
fn fmt_mins(mins: u64) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(start: &str, end: &str) -> GrantScheduleWindow {
        GrantScheduleWindow {
            start: start.into(),
            end: end.into(),
        }
    }

    fn schedule(weekly: WeeklySchedule) -> GrantSchedule {
        GrantSchedule {
            v: 1,
            tz: "Europe/London".into(),
            paused: None,
            weekly,
            overrides: None,
            issued_at: 0,
        }
    }

    #[test]
    fn week_lines_group_identical_consecutive_days() {
        let weekday = Some(vec![win("07:00", "20:00")]);
        let weekend = Some(vec![win("08:00", "21:00")]);
        let s = schedule(WeeklySchedule {
            mon: weekday.clone(),
            tue: weekday.clone(),
            wed: weekday.clone(),
            thu: weekday.clone(),
            fri: weekday,
            sat: weekend.clone(),
            sun: weekend,
        });
        assert_eq!(
            week_lines(&s.weekly),
            vec!["Mon–Fri   07:00 – 20:00", "Sat–Sun   08:00 – 21:00"]
        );
    }

    #[test]
    fn week_lines_name_a_lone_day_and_empty_days() {
        let s = schedule(WeeklySchedule {
            mon: Some(vec![win("07:00", "20:00")]),
            ..Default::default()
        });
        assert_eq!(
            week_lines(&s.weekly),
            vec!["Mon   07:00 – 20:00", "Tue–Sun   no screen time"]
        );
    }

    #[test]
    fn back_at_says_today_tomorrow_or_weekday() {
        // 2026-07-04 12:00 UTC is a Saturday; Europe/London is UTC+1 (13:00).
        let sat_noon = 1783166400; // 2026-07-04 12:00:00 UTC
        let tz: chrono_tz::Tz = "Europe/London".parse().unwrap();
        // +4h -> 17:00 London, same day.
        assert_eq!(
            back_at_line(tz, sat_noon, 4 * 3600),
            "You can come back at 17:00 today."
        );
        // +18h -> 07:00 London tomorrow (Sunday).
        assert_eq!(
            back_at_line(tz, sat_noon, 18 * 3600),
            "You can come back at 07:00 tomorrow."
        );
        // +42h -> Monday 07:00.
        assert_eq!(
            back_at_line(tz, sat_noon, 42 * 3600),
            "You can come back on Mon at 07:00."
        );
    }

    #[test]
    fn lock_info_combines_detail_week_and_budget() {
        let s = schedule(WeeklySchedule {
            mon: Some(vec![win("07:00", "20:00")]),
            tue: Some(vec![win("07:00", "20:00")]),
            wed: Some(vec![win("07:00", "20:00")]),
            thu: Some(vec![win("07:00", "20:00")]),
            fri: Some(vec![win("07:00", "20:00")]),
            sat: Some(vec![win("07:00", "20:00")]),
            sun: Some(vec![win("07:00", "20:00")]),
        });
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: "Europe/London".into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: None,
            paused: None,
            revoked: None,
            issued_at: 0,
        };
        let info = lock_info(Some(&s), Some(&b), Some(4 * 3600), 65 * 60, 1783166400);
        assert!(info.detail.unwrap().contains("today"));
        assert_eq!(info.lines.len(), 2);
        assert_eq!(info.lines[0], "Mon–Sun   07:00 – 20:00");
        assert_eq!(
            info.lines[1],
            "Screen time: up to 2h a day — 1h 05m used today"
        );
    }

    /// A device that is off until a guardian opens it says so once, in the
    /// title. Listing seven days of "no screen time" underneath reads as a
    /// half-finished setup — the impression the posture exists to correct.
    #[test]
    fn a_dormant_schedule_lists_no_week_at_all() {
        let mut s = schedule(WeeklySchedule::default());
        s.paused = Some(true);
        let info = lock_info(Some(&s), None, None, 0, 1783166400);
        assert!(info.lines.is_empty());
        // It overrides the headline too: "Outside allowed hours" describes a
        // device that HAS allowed hours.
        assert_eq!(info.title.as_deref(), Some("This device is off"));
        // The reason names the way back on — a person, not an hour. Crucially
        // it is never a "come back at…" line: there is no hour to name.
        let detail = info.detail.expect("dormant gets an honest reason");
        assert!(detail.contains("guardian"));
        assert!(!detail.contains("come back"));
    }

    #[test]
    fn fmt_mins_reads_naturally() {
        assert_eq!(fmt_mins(45), "45m");
        assert_eq!(fmt_mins(120), "2h");
        assert_eq!(fmt_mins(65), "1h 05m");
    }
}
