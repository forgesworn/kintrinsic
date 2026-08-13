//! What the tray shows and says. Deliberately calm, companion-toned copy: the
//! tray is reflection ("here's where you stand, here's the lever to ask"),
//! never surveillance or nagging.

use charter_ipc::dto::{DaemonEvent, Op, ReqState, TimeLeftView};

/// How the icon should read at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Time in hand.
    Ok,
    /// Running low — worth glancing at.
    Low,
    /// Locked (any reason).
    Locked,
    /// The daemon is unreachable; nothing is being tracked or enforced here.
    Unknown,
}

/// The rendered tray state: icon posture + the menu's header line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub icon: IconState,
    /// Tooltip / accessible title.
    pub title: String,
    /// The disabled header row at the top of the menu.
    pub status_line: String,
    /// Whether the "Ask for more time" entries are offered.
    pub asks_enabled: bool,
    /// The hover tooltip's second line: the SHAPE of the time left, in one
    /// breath — how much today, how long until the day's hours close, and how
    /// much of the week remains when a weekly cap is set.
    ///
    /// Separate from `title` because a tooltip is read at a glance, in
    /// passing, by someone who has not clicked anything. It answers "have I
    /// got time for this?" without opening a window. Empty when there is
    /// nothing further to say beyond the title.
    pub tooltip_detail: String,
}

/// A desktop notification the shell should raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    /// Critical urgency (stays on screen until dismissed).
    pub urgent: bool,
}

/// Below this many minutes the icon turns "low".
const LOW_MINUTES: i64 = 15;

pub(crate) fn humanize(secs: i64) -> String {
    let m = (secs.max(0) + 59) / 60; // round up: "1m" until it is truly 0
    if m >= 60 {
        let (h, r) = (m / 60, m % 60);
        if r == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {r:02}m")
        }
    } else {
        format!("{m}m")
    }
}

/// The tooltip's detail line: the SHAPE of what is left, at a glance.
///
/// Built from the parts rather than the binding minimum, because "40 minutes
/// left" alone hides whether that is the day's allowance running out or the
/// evening closing — and those need different plans. Clauses are joined with
/// " · " and only the ones actually set appear, so a ward with a plain daily
/// limit gets one short phrase rather than a row of "not set".
pub fn tooltip_detail_for(t: &TimeLeftView) -> String {
    detail_parts_for(t).join(" · ")
}

/// The same shape as [`tooltip_detail_for`], before it is joined into one
/// breath. The menu wants these as separate rows — a tooltip is read in
/// passing and wants one line — so the parts are built once, here, and the
/// two surfaces cannot come to disagree about what the day looks like.
pub(crate) fn detail_parts_for(t: &TimeLeftView) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    if t.budget_day_seconds >= 0 {
        parts.push(format!("{} left today", humanize(t.budget_day_seconds)));
    }
    if t.budget_week_seconds >= 0 {
        parts.push(format!(
            "{} left this week",
            humanize(t.budget_week_seconds)
        ));
    }
    // The window is a wall-clock deadline, not an allowance, so it is phrased
    // as one: "until the day's hours end", never "left".
    if t.schedule_seconds > 0 {
        parts.push(format!(
            "{} until the day's hours end",
            humanize(t.schedule_seconds)
        ));
    }
    parts
}

/// The tray state for a time-left snapshot (`None` = daemon unreachable).
pub fn view_for(t: Option<&TimeLeftView>) -> TrayView {
    let Some(t) = t else {
        return TrayView {
            icon: IconState::Unknown,
            title: "Kintrinsic — not reachable".into(),
            status_line: "Kintrinsic isn't reachable right now.".into(),
            asks_enabled: false,
            tooltip_detail: "Nothing is being tracked or kept here right now.".into(),
        };
    };
    // The daemon's fail-safe `unknown()` answer — served to unmanaged callers
    // (a parent's own account) and before the first enforcement tick — reads
    // locked with reason "unknown". That is "no signal", not a lock:
    // rendering a parent's own desktop as "Outside allowed hours" would be a
    // lie. A real lock always names a real wall.
    if t.locked && matches!(t.reason.as_deref(), None | Some("unknown")) {
        return TrayView {
            icon: IconState::Unknown,
            title: "Kintrinsic — not watching this account".into(),
            status_line: "Kintrinsic isn't watching this account.".into(),
            asks_enabled: false,
            tooltip_detail: "No limits are kept for this account.".into(),
        };
    }
    if t.locked {
        // Name the wall that was met, same rule as every lock surface: a
        // budget waits for tomorrow, a window waits for the window, a
        // stand-down waits for a person.
        let status_line = match t.reason.as_deref() {
            Some("standdown") => {
                "That's it for today — your guardian asked you to finish up.".into()
            }
            Some("budget") => "Time's up for today.".into(),
            _ => match t.next_open {
                Some(_) => "Outside allowed hours — you can come back later.".into(),
                None => "Outside allowed hours right now.".into(),
            },
        };
        // On a lock the detail says the way BACK, not the arithmetic: a ward
        // staring at a locked screen needs "when", and the remaining-time
        // parts are all zero anyway.
        let tooltip_detail = match t.reason.as_deref() {
            Some("standdown") => "Locked until your guardian lifts it.".into(),
            Some("budget") => "Your time starts again tomorrow.".into(),
            _ => "Click to see when you can come back.".into(),
        };
        return TrayView {
            icon: IconState::Locked,
            title: "Kintrinsic — locked".into(),
            status_line,
            asks_enabled: true,
            tooltip_detail,
        };
    }
    if t.effective_seconds < 0 {
        return TrayView {
            icon: IconState::Ok,
            title: "Kintrinsic — no limit today".into(),
            status_line: "No time limit today.".into(),
            asks_enabled: false,
            tooltip_detail: "No screen-time limit is set for today.".into(),
        };
    }
    let left = humanize(t.effective_seconds);
    TrayView {
        icon: if t.effective_seconds < LOW_MINUTES * 60 {
            IconState::Low
        } else {
            IconState::Ok
        },
        title: format!("Kintrinsic — {left} left"),
        status_line: format!("{left} left today"),
        asks_enabled: true,
        tooltip_detail: tooltip_detail_for(t),
    }
}

/// The `time.extend` request params for an ask of `minutes`, routed to the
/// wall that is actually hit (M7 — see [`TimeLeftView::limit_hit`]).
pub fn ask_params(t: Option<&TimeLeftView>, minutes: u16) -> String {
    let limit_hit = t.map(TimeLeftView::limit_hit).unwrap_or("budget");
    serde_json::json!({ "minutesRequested": minutes, "limitHit": limit_hit }).to_string()
}

/// The `time.extend` request params for asking to top up ONE named group —
/// explicit `bucketId` + `limitHit: "bucket"`, so the daemon credits THAT
/// allowance's own pool, never the device's (the same M7 discipline
/// [`ask_params`] follows for the whole-device wall, one level down).
pub fn bucket_ask_params(bucket_id: &str, minutes: u16) -> String {
    serde_json::json!({
        "minutesRequested": minutes,
        "limitHit": "bucket",
        "bucketId": bucket_id,
    })
    .to_string()
}

/// The `app.open` request params for asking to open (or hold open) one
/// `askFirst` app.
pub fn app_open_params(pkg: &str, label: &str) -> String {
    serde_json::json!({ "pkg": pkg, "label": label }).to_string()
}

/// One thing the ward clicked that the daemon should hear about. Carries
/// only what each ask needs to build its own request — never a whole
/// `TimeLeftView` — so both shells (the generic StatusNotifierItem tray and
/// Mint's xapp tray) wire the SAME three actions the exact same way, and the
/// wire params + the "asked!" toast can be built once, here, from the
/// action alone (see [`submit_for`] / [`asked_notice_for`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAsk {
    /// The whole-device wall ("Ask for N more minutes"). Routed against the
    /// snapshot live at submit time (M7 — see [`ask_params`]), so this
    /// carries only the minutes, not pre-built params.
    Device(u16),
    /// One named group's own "ask for more". `label` is carried purely for
    /// the toast — the wire params only ever need the id.
    Bucket {
        bucket_id: String,
        minutes: u16,
        label: String,
    },
    /// One `askFirst` app's "ask to open".
    AppOpen { pkg: String, label: String },
}

/// The `(op, params_json)` to submit for a clicked ask. `latest` is only
/// consulted for [`TrayAsk::Device`] (M7 routing); the other two are
/// self-contained.
pub fn submit_for(ask: &TrayAsk, latest: Option<&TimeLeftView>) -> (Op, String) {
    match ask {
        TrayAsk::Device(minutes) => (Op::TimeExtend, ask_params(latest, *minutes)),
        TrayAsk::Bucket {
            bucket_id, minutes, ..
        } => (Op::TimeExtend, bucket_ask_params(bucket_id, *minutes)),
        TrayAsk::AppOpen { pkg, label } => (Op::AppOpen, app_open_params(pkg, label)),
    }
}

/// The "your ask is on its way" toast for a just-submitted ask — the SAME
/// words on every shell, so a click reads the same regardless of desktop.
pub fn asked_notice_for(ask: &TrayAsk) -> Notice {
    let body = match ask {
        TrayAsk::Device(minutes) => {
            format!("Your ask for {minutes} more minutes is on its way to your guardian.")
        }
        TrayAsk::Bucket { label, .. } => {
            format!("Your ask for more {label} time is on its way to your guardian.")
        }
        TrayAsk::AppOpen { label, .. } => {
            format!("Your ask to open {label} is on its way to your guardian.")
        }
    };
    Notice {
        summary: "Asked!".into(),
        body,
        urgent: false,
    }
}

/// The notification (if any) a daemon event deserves. Only the ward's own
/// asks' outcomes notify — the tray tells the child the ANSWER, it does not
/// narrate enforcement. `time.extend` covers both the whole-device ask and a
/// named group's own "ask for more" (the wire carries no bucket id on the
/// status row, so both read the same generic "yes/no" — which is the whole
/// answer either way); `app.open` gets its own copy.
pub fn notice_for(ev: &DaemonEvent) -> Option<Notice> {
    let DaemonEvent::RequestUpdated { status } = ev else {
        return None;
    };
    match status.op {
        Op::TimeExtend => match status.state {
            ReqState::Granted | ReqState::Enacted => Some(Notice {
                summary: "More time — yes!".into(),
                body: "Your guardian said yes. The extra time is on its way to your meter.".into(),
                urgent: false,
            }),
            ReqState::Denied => Some(Notice {
                summary: "Not this time".into(),
                body: "Your guardian said not right now. You can always ask again — or better, ask in person.".into(),
                urgent: false,
            }),
            ReqState::Expired => Some(Notice {
                summary: "Your ask expired".into(),
                body: "Nobody answered in time. Try asking again.".into(),
                urgent: false,
            }),
            // The in-between machinery states stay quiet, and a cancelled ask
            // was the ward's own act — nothing to announce.
            ReqState::Pending
            | ReqState::Enacting
            | ReqState::Failed
            | ReqState::Rejected
            | ReqState::Cancelled => None,
        },
        Op::AppOpen => match status.state {
            ReqState::Granted | ReqState::Enacted => Some(Notice {
                summary: "You can open it!".into(),
                body: "Your guardian said yes — go ahead.".into(),
                urgent: false,
            }),
            ReqState::Denied => Some(Notice {
                summary: "Not this time".into(),
                body: "Your guardian said not right now. You can always ask again — or better, ask in person.".into(),
                urgent: false,
            }),
            ReqState::Expired => Some(Notice {
                summary: "Your ask expired".into(),
                body: "Nobody answered in time. Try asking again.".into(),
                urgent: false,
            }),
            ReqState::Pending
            | ReqState::Enacting
            | ReqState::Failed
            | ReqState::Rejected
            | ReqState::Cancelled => None,
        },
        // Every other op (install, exec.allow, …) is not the tray's story to
        // tell — same as before.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_ipc::dto::RequestStatusView;

    // ---- the hover tooltip's detail line ---------------------------------

    fn shaped(day: i64, week: i64, schedule: i64) -> TimeLeftView {
        TimeLeftView {
            effective_seconds: [day, week, schedule]
                .into_iter()
                .filter(|v| *v >= 0)
                .min()
                .unwrap_or(-1),
            schedule_seconds: schedule,
            budget_seconds: [day, week]
                .into_iter()
                .filter(|v| *v >= 0)
                .min()
                .unwrap_or(-1),
            extension_seconds: 0,
            locked: false,
            reason: None,
            next_open: None,
            offline: false,
            learning_today_seconds: None,
            used_today_seconds: None,
            buckets: Vec::new(),
            budget_day_seconds: day,
            budget_week_seconds: week,
            ask_first: Vec::new(),
        }
    }

    /// A plain daily limit says one short thing, not a table of "not set".
    #[test]
    fn the_tooltip_names_only_the_walls_that_exist() {
        assert_eq!(
            tooltip_detail_for(&shaped(46 * 60, -1, -1)),
            "46m left today"
        );
    }

    /// A weekly cap is its own sentence — "you have used this week up" needs a
    /// different plan from "come back tomorrow".
    #[test]
    fn the_tooltip_says_the_week_when_a_weekly_cap_is_set() {
        let d = tooltip_detail_for(&shaped(46 * 60, 5 * 3600, -1));
        assert_eq!(d, "46m left today · 5h left this week");
    }

    /// The window is a deadline, not an allowance, so it is phrased as one.
    #[test]
    fn the_tooltip_phrases_the_window_as_a_deadline() {
        let d = tooltip_detail_for(&shaped(46 * 60, -1, 2 * 3600 + 35 * 60));
        assert_eq!(d, "46m left today · 2h 35m until the day's hours end");
    }

    /// All three, in the order a ward reads them: mine today, mine this week,
    /// then the clock on the wall.
    #[test]
    fn the_tooltip_carries_all_three_walls_when_all_are_set() {
        let d = tooltip_detail_for(&shaped(46 * 60, 5 * 3600, 2 * 3600));
        assert_eq!(
            d,
            "46m left today · 5h left this week · 2h until the day's hours end"
        );
    }

    /// A ward with nothing set gets nothing rather than an empty scaffold.
    #[test]
    fn the_tooltip_is_empty_when_no_wall_is_set() {
        assert_eq!(tooltip_detail_for(&shaped(-1, -1, -1)), "");
    }

    /// A LOCKED ward is told the way back, not zeroes: every remaining figure
    /// is 0 at that point, and "0m left today" is not something to act on.
    #[test]
    fn a_locked_tray_offers_the_way_back_not_the_arithmetic() {
        let v = view_for(Some(&t(0, true, Some("budget"))));
        assert_eq!(v.tooltip_detail, "Your time starts again tomorrow.");
        let v = view_for(Some(&t(0, true, Some("standdown"))));
        assert_eq!(v.tooltip_detail, "Locked until your guardian lifts it.");
    }

    /// The daemon's fail-safe "unknown" (an unmanaged account, or before the
    /// first tick) must never read as a lock in the tooltip either.
    #[test]
    fn an_unwatched_account_says_so_in_the_tooltip() {
        let v = view_for(Some(&t(0, true, Some("unknown"))));
        assert_eq!(v.tooltip_detail, "No limits are kept for this account.");
        assert_eq!(view_for(None).icon, IconState::Unknown);
    }

    fn t(effective: i64, locked: bool, reason: Option<&str>) -> TimeLeftView {
        TimeLeftView {
            effective_seconds: effective,
            schedule_seconds: effective,
            budget_seconds: effective,
            extension_seconds: 0,
            locked,
            reason: reason.map(|r| r.into()),
            next_open: None,
            offline: false,
            learning_today_seconds: None,
            used_today_seconds: None,
            buckets: Vec::new(),
            budget_day_seconds: -1,
            budget_week_seconds: -1,
            ask_first: Vec::new(),
        }
    }

    #[test]
    fn a_healthy_afternoon_reads_as_time_in_hand() {
        let v = view_for(Some(&t(90 * 60, false, None)));
        assert_eq!(v.icon, IconState::Ok);
        assert_eq!(v.status_line, "1h 30m left today");
        assert!(v.asks_enabled);
    }

    #[test]
    fn running_low_changes_the_posture_not_the_offer() {
        let v = view_for(Some(&t(10 * 60, false, None)));
        assert_eq!(v.icon, IconState::Low);
        assert!(v.asks_enabled);
    }

    /// The tray names the wall, like every other lock surface — and a
    /// stand-down names the PERSON, because a person is the way back.
    #[test]
    fn a_lock_names_its_wall() {
        let sd = view_for(Some(&t(0, true, Some("standdown"))));
        assert_eq!(sd.icon, IconState::Locked);
        assert!(sd.status_line.contains("guardian asked you to finish up"));

        let budget = view_for(Some(&t(0, true, Some("budget"))));
        assert!(budget.status_line.contains("Time's up"));

        let schedule = view_for(Some(&t(0, true, Some("schedule"))));
        assert!(schedule.status_line.contains("allowed hours"));
        // Locked is exactly when asking matters most.
        assert!(sd.asks_enabled && budget.asks_enabled && schedule.asks_enabled);
    }

    /// An unreachable daemon must read as "not tracked", never as "no limit" —
    /// and must not offer an ask that has nowhere to go.
    #[test]
    fn unreachable_is_not_unlimited() {
        let v = view_for(None);
        assert_eq!(v.icon, IconState::Unknown);
        assert!(!v.asks_enabled);
    }

    /// The daemon answers an UNMANAGED caller (a parent's own account, an
    /// account with no charter) with the fail-safe `unknown()` snapshot —
    /// locked, but with NO reason. The tray must read that as "not watched
    /// here", never render a parent's own desktop as "Outside allowed hours".
    #[test]
    fn an_unmanaged_account_is_not_told_it_is_locked() {
        let v = view_for(Some(&TimeLeftView::unknown()));
        assert_eq!(v.icon, IconState::Unknown);
        assert!(!v.asks_enabled);
        assert!(
            !v.status_line.to_lowercase().contains("hours"),
            "must not read as a real lock: {}",
            v.status_line
        );
    }

    #[test]
    fn an_unbounded_day_offers_nothing_to_ask_for() {
        let v = view_for(Some(&t(-1, false, None)));
        assert_eq!(v.icon, IconState::Ok);
        assert!(!v.asks_enabled);
        assert!(v.status_line.contains("No time limit"));
    }

    /// Sub-minute remainders round UP — "1m" until it is truly gone, so the
    /// tray never shows "0m left" while the screen still works.
    #[test]
    fn the_last_seconds_still_read_as_a_minute() {
        let v = view_for(Some(&t(30, false, None)));
        assert_eq!(v.status_line, "1m left today");
    }

    /// M7 rides along: an ask made under a schedule lock names "schedule".
    #[test]
    fn an_ask_names_the_wall_it_is_pushing_on() {
        let mut bedtime = t(0, true, Some("schedule"));
        bedtime.budget_seconds = 1800;
        let p = ask_params(Some(&bedtime), 30);
        assert!(p.contains("\"limitHit\":\"schedule\""));
        assert!(p.contains("\"minutesRequested\":30"));
        // No snapshot at all defaults to budget, mirroring the CLI.
        assert!(ask_params(None, 15).contains("\"limitHit\":\"budget\""));
    }

    /// A group's own ask names ITS wall — `bucket` — and carries the
    /// specific id, so the daemon credits that allowance's own pool.
    #[test]
    fn a_bucket_ask_names_its_own_group() {
        let p = bucket_ask_params("play", 15);
        assert!(p.contains("\"limitHit\":\"bucket\""), "{p}");
        assert!(p.contains("\"bucketId\":\"play\""), "{p}");
        assert!(p.contains("\"minutesRequested\":15"), "{p}");
    }

    /// An "ask to open" carries the app's identity and its display label.
    #[test]
    fn an_app_open_ask_carries_pkg_and_label() {
        let p = app_open_params("com.mojang.minecraftpe", "Minecraft");
        assert!(p.contains("\"pkg\":\"com.mojang.minecraftpe\""), "{p}");
        assert!(p.contains("\"label\":\"Minecraft\""), "{p}");
    }

    /// Each `TrayAsk` maps to the right op + params — the routing a shell's
    /// click handler leans on entirely, so it never has to know the wire
    /// shapes itself.
    #[test]
    fn submit_for_routes_each_ask_to_its_op_and_params() {
        let (op, params) = submit_for(&TrayAsk::Device(20), Some(&t(600, false, None)));
        assert_eq!(op, Op::TimeExtend);
        assert!(params.contains("\"minutesRequested\":20"), "{params}");

        let (op, params) = submit_for(
            &TrayAsk::Bucket {
                bucket_id: "play".into(),
                minutes: 15,
                label: "Play".into(),
            },
            None,
        );
        assert_eq!(op, Op::TimeExtend);
        assert!(params.contains("\"bucketId\":\"play\""), "{params}");

        let (op, params) = submit_for(
            &TrayAsk::AppOpen {
                pkg: "com.mojang.minecraftpe".into(),
                label: "Minecraft".into(),
            },
            None,
        );
        assert_eq!(op, Op::AppOpen);
        assert!(
            params.contains("\"pkg\":\"com.mojang.minecraftpe\""),
            "{params}"
        );
    }

    /// The "asked!" toast names what was actually asked for, on every kind
    /// of ask.
    #[test]
    fn asked_notice_names_what_was_asked_for() {
        assert!(asked_notice_for(&TrayAsk::Device(20))
            .body
            .contains("20 more minutes"));
        assert!(asked_notice_for(&TrayAsk::Bucket {
            bucket_id: "play".into(),
            minutes: 15,
            label: "Play".into(),
        })
        .body
        .contains("more Play time"));
        assert!(asked_notice_for(&TrayAsk::AppOpen {
            pkg: "com.mojang.minecraftpe".into(),
            label: "Minecraft".into(),
        })
        .body
        .contains("open Minecraft"));
    }

    fn req(state: ReqState, op: Op) -> DaemonEvent {
        DaemonEvent::RequestUpdated {
            status: RequestStatusView {
                req_id: "r1".into(),
                op,
                state,
                detail: None,
                created_at: 0,
            },
        }
    }

    #[test]
    fn only_answers_become_notifications() {
        assert!(notice_for(&req(ReqState::Granted, Op::TimeExtend))
            .is_some_and(|n| n.summary.contains("yes")));
        assert!(notice_for(&req(ReqState::Denied, Op::TimeExtend))
            .is_some_and(|n| n.body.contains("ask again")));
        assert!(notice_for(&req(ReqState::Expired, Op::TimeExtend)).is_some());
        // The in-between machinery states stay quiet…
        assert!(notice_for(&req(ReqState::Pending, Op::TimeExtend)).is_none());
        assert!(notice_for(&req(ReqState::Enacting, Op::TimeExtend)).is_none());
        // …and other ops are not the tray's story to tell.
        assert!(notice_for(&req(ReqState::Granted, Op::InstallFlatpak)).is_none());
        // Nor are lock transitions — the lock surface itself owns those.
        assert!(notice_for(&DaemonEvent::LockStateChanged {
            locked: true,
            reason: Some("standdown".into())
        })
        .is_none());
    }

    /// An "ask to open" answers with its own copy — allow reads as an
    /// invitation to go ahead, not a repeat of the whole-device "more time".
    #[test]
    fn app_open_answers_get_their_own_copy() {
        assert!(notice_for(&req(ReqState::Granted, Op::AppOpen))
            .is_some_and(|n| n.summary.contains("open")));
        assert!(notice_for(&req(ReqState::Denied, Op::AppOpen))
            .is_some_and(|n| n.body.contains("ask again")));
        assert!(notice_for(&req(ReqState::Expired, Op::AppOpen)).is_some());
        assert!(notice_for(&req(ReqState::Pending, Op::AppOpen)).is_none());
        assert!(notice_for(&req(ReqState::Cancelled, Op::AppOpen)).is_none());
    }
}
