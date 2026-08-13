//! The tray menu as DATA, so two shells cannot drift apart.
//!
//! Mint draws its own menu (`XAppStatusIcon::set_primary_menu`), every other
//! desktop gets the StatusNotifierItem menu. Both render this same list, so the
//! copy a ward reads lives here — in the tested library, inside the CI gate —
//! and not twice in two rendering shells.

use crate::model::{detail_parts_for, humanize, view_for};
use charter_ipc::dto::{BucketView, TimeLeftView};

/// The minute offers on the menu. Fixed entries rather than a slider: a menu
/// cannot hold a draggable one reliably on either shell.
pub const ASK_MINUTES: [u16; 3] = [15, 30, 60];

/// What a spent group's own "ask for more" asks for. A single fixed amount
/// rather than the whole-device's three options — a menu that offered three
/// minute choices per named allowance would grow a row for every group in
/// the family; this is a lightweight nudge, not a negotiation.
pub const BUCKET_ASK_MINUTES: u16 = ASK_MINUTES[0];

/// One row of the tray menu.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuRow {
    /// The headline, not clickable. `bar` is how much of the day is already
    /// spent (0.0..=1.0), when there is a day allowance to draw.
    Header {
        label: String,
        bar: Option<f64>,
    },
    /// A plain line of detail. Not clickable.
    Line(String),
    Separator,
    /// One named allowance, with its own meter.
    Bucket {
        label: String,
        detail: String,
        bar: Option<f64>,
    },
    /// "Ask for N more minutes."
    Ask {
        minutes: u16,
        label: String,
        enabled: bool,
    },
    /// "Ask for more Play time" — a spent named allowance's own ask. Carries
    /// the bucket id so the daemon credits THAT allowance's own pool, never
    /// the device's — the same M7 discipline `Ask` follows for the
    /// whole-device wall, one level down. `group_label` is the group's own
    /// name ("Play"), kept apart from the full menu sentence so a shell can
    /// build the ask + its toast without parsing a sentence back apart.
    AskBucket {
        bucket_id: String,
        minutes: u16,
        group_label: String,
        label: String,
    },
    /// "Ask to open Minecraft" — one `askFirst` app still gated right now.
    /// `app_label` is the app's own name, same reasoning as `group_label`
    /// above.
    AskToOpen {
        pkg: String,
        app_label: String,
        label: String,
    },
    /// The way into the full app.
    OpenApp {
        label: String,
    },
}

/// The whole menu, from a time-left snapshot (`None` = daemon unreachable).
///
/// Reuses [`view_for`] for the headline so the tray's icon, its tooltip and
/// its menu cannot say three different things about the same moment.
pub fn menu_rows(t: Option<&TimeLeftView>) -> Vec<MenuRow> {
    let view = view_for(t);
    let mut rows = vec![MenuRow::Header {
        label: view.status_line.clone(),
        bar: t.and_then(day_fraction_spent),
    }];

    // Detail. While time runs this is the shape of the day, one wall per row;
    // on a lock (or an unwatched account) the view has already replaced the
    // arithmetic with the way back, which is a single sentence.
    match t.filter(|t| !t.locked) {
        Some(t) => rows.extend(
            detail_parts_for(t)
                .into_iter()
                // The header already said the day. Saying it twice is how a
                // menu starts to read like a machine wrote it.
                .filter(|p| *p != view.status_line)
                .map(MenuRow::Line),
        ),
        None => {
            if !view.tooltip_detail.is_empty() {
                rows.push(MenuRow::Line(view.tooltip_detail.clone()));
            }
        }
    }

    // The named allowances, each with its own day.
    let buckets = t.map(|t| t.buckets.as_slice()).unwrap_or(&[]);
    if !buckets.is_empty() {
        rows.push(MenuRow::Separator);
        for b in buckets {
            // A lifted bucket still shows — its meter runs and the ward should
            // see it — but nothing is being taken, so there is no wall to
            // count down to and no bar to draw against.
            let (detail, bar) = if !b.capped {
                ("no limit right now".to_string(), None)
            } else if b.spent {
                ("used up".to_string(), Some(1.0))
            } else {
                (bucket_time_detail(b), bucket_bar_fraction(b))
            };
            rows.push(MenuRow::Bucket {
                label: b.label.clone(),
                detail,
                bar,
            });
            // A spent group is the one moment "ask for more" of it makes
            // sense: a group still running has nothing to ask for yet, and a
            // lifted (uncapped) group has no wall to push on at all.
            if b.capped && b.spent {
                rows.push(MenuRow::AskBucket {
                    bucket_id: b.id.clone(),
                    minutes: BUCKET_ASK_MINUTES,
                    group_label: b.label.clone(),
                    label: format!("Ask for more {} time", b.label),
                });
            }
        }
    }

    // Apps presently gated by `blocked`/`askFirst` — an "ask to open" instead
    // of a flat wall. Only ever populated when the daemon's `apps` clause
    // actually flags one, so an ordinary family sees no such section at all.
    let ask_first = t.map(|t| t.ask_first.as_slice()).unwrap_or(&[]);
    if !ask_first.is_empty() {
        rows.push(MenuRow::Separator);
        for a in ask_first {
            rows.push(MenuRow::AskToOpen {
                pkg: a.pkg.clone(),
                app_label: a.label.clone(),
                label: format!("Ask to open {}", a.label),
            });
        }
    }

    rows.push(MenuRow::Separator);
    for minutes in ASK_MINUTES {
        rows.push(MenuRow::Ask {
            minutes,
            label: format!("Ask for {minutes} more minutes"),
            enabled: view.asks_enabled,
        });
    }
    rows.push(MenuRow::Separator);
    rows.push(MenuRow::OpenApp {
        label: "Open Kintrinsic…".into(),
    });
    rows
}

/// A capped, not-yet-spent bucket's own bar fraction (`0.0..=1.0`), or `None`
/// when there's no meaningful wall to measure against (`limit_seconds == 0`).
///
/// `used_seconds` is ALWAYS the raw DAY meter (today's usage only) —
/// `charterd::bucket_view`'s own doc. For a bucket with a day cap of its own,
/// that is exactly the right numerator for `limit_seconds` (also the day
/// wall). But a bucket with NO day cap (weekly-only) has its day slot filled
/// from the week's own numbers instead (the same "binding wall" fallback
/// `bucket_time_detail` already keys off — `limit_seconds == week_limit_
/// seconds` is the tell), so dividing TODAY's few minutes by the WEEK's
/// multi-hour wall drew a nearly-empty bar even at, say, 4 of a 5-hour week
/// (found in review). For that case the bar is driven off the week meter/wall
/// pair instead; a day-capped bucket (with or without a week cap alongside
/// it) is unaffected — its own day meter over its own day wall, unchanged.
fn bucket_bar_fraction(b: &BucketView) -> Option<f64> {
    if b.week_limit_seconds >= 0 && b.limit_seconds == b.week_limit_seconds as u64 {
        let week_remaining = b.week_remaining_seconds.clamp(0, b.week_limit_seconds);
        let week_used = (b.week_limit_seconds - week_remaining) as f64;
        (b.week_limit_seconds > 0)
            .then(|| (week_used / b.week_limit_seconds as f64).clamp(0.0, 1.0))
    } else {
        (b.limit_seconds > 0)
            .then(|| (b.used_seconds as f64 / b.limit_seconds as f64).clamp(0.0, 1.0))
    }
}

/// The non-spent, non-paused detail line for one bucket: the day remainder
/// alone, or joined with the week remainder — " · " like the tooltip's own
/// join — when a weekly cap is ALSO set on this group.
///
/// A bucket with no DAY cap of its own (weekly-only) has its day slot filled
/// from the week's own numbers by the daemon (the "binding wall" fallback —
/// `charterd::bucket_view`), so `remaining_seconds` and `week_remaining_
/// seconds` read identical in that case. Saying the same figure twice as
/// "45m left today · 45m left this week" would read as a menu that doesn't
/// know its own numbers, so that case collapses to the one true sentence.
fn bucket_time_detail(b: &BucketView) -> String {
    if b.week_limit_seconds < 0 {
        return format!("{} left", humanize(b.remaining_seconds as i64));
    }
    let week = humanize(b.week_remaining_seconds.max(0));
    if b.week_remaining_seconds == b.remaining_seconds as i64 {
        return format!("{week} left this week");
    }
    format!(
        "{} left today · {week} left this week",
        humanize(b.remaining_seconds as i64)
    )
}

/// How much of today's allowance is already spent, as `0.0..=1.0`.
///
/// `None` unless BOTH figures are real: without "used" there is no honest
/// fraction, and without a daily cap there is no wall to draw against. A bar
/// invented from one of them would be a claim about the size of the day that
/// nobody made.
fn day_fraction_spent(t: &TimeLeftView) -> Option<f64> {
    let used = t.used_today_seconds? as f64;
    if t.budget_day_seconds < 0 {
        return None;
    }
    let total = used + t.budget_day_seconds as f64;
    (total > 0.0).then(|| (used / total).clamp(0.0, 1.0))
}

/// Whether this desktop should be drawn by Mint's own icon API rather than the
/// generic StatusNotifierItem tray.
///
/// Decided from `XDG_CURRENT_DESKTOP`, never by probing the session bus: the
/// panel claims its bus name some time after login, so a probe would race the
/// desktop coming up — the exact failure that made 0.6.0's tray invisible. The
/// variable is set before autostart runs and cannot race.
///
/// Generous on purpose. Mint's MATE and XFCE editions use the same icon API,
/// and on a NON-Mint MATE or XFCE the library simply isn't there, so the shell
/// falls back on its own.
pub fn prefers_xapp_shell(xdg_current_desktop: &str) -> bool {
    xdg_current_desktop.split(':').any(|d| {
        let d = d.trim().to_ascii_lowercase();
        matches!(d.as_str(), "cinnamon" | "x-cinnamon" | "mate" | "xfce")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_ipc::dto::BucketView;

    fn day_only(left_secs: i64, used_secs: Option<u64>) -> TimeLeftView {
        TimeLeftView {
            effective_seconds: left_secs,
            schedule_seconds: -1,
            budget_seconds: left_secs,
            extension_seconds: 0,
            locked: false,
            reason: None,
            next_open: None,
            offline: false,
            learning_today_seconds: None,
            used_today_seconds: used_secs,
            buckets: Vec::new(),
            budget_day_seconds: left_secs,
            budget_week_seconds: -1,
            ask_first: Vec::new(),
        }
    }

    fn labels(rows: &[MenuRow]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                MenuRow::Header { label, .. } => label.clone(),
                MenuRow::Line(s) => s.clone(),
                MenuRow::Separator => "---".into(),
                MenuRow::Bucket { label, detail, .. } => format!("{label} · {detail}"),
                MenuRow::Ask { label, .. } => label.clone(),
                MenuRow::AskBucket { label, .. } => label.clone(),
                MenuRow::AskToOpen { label, .. } => label.clone(),
                MenuRow::OpenApp { label } => label.clone(),
            })
            .collect()
    }

    /// The headline is the same sentence the rest of Kintrinsic uses, and the
    /// bar under it is how much of the day has gone.
    #[test]
    fn the_headline_carries_the_day_and_its_meter() {
        let t = day_only(46 * 60, Some(74 * 60));
        let rows = menu_rows(Some(&t));
        match &rows[0] {
            MenuRow::Header { label, bar } => {
                assert_eq!(label, "46m left today");
                // 74 used of 120 = the bar is most of the way along.
                let b = bar.expect("a day allowance draws a bar");
                assert!((b - 74.0 / 120.0).abs() < 0.001, "bar was {b}");
            }
            other => panic!("first row must be the headline, got {other:?}"),
        }
    }

    /// Without a `used today` figure there is no honest fraction to draw, so
    /// nothing is drawn. A half-full bar invented from nothing is a lie about
    /// how big the day was.
    #[test]
    fn no_used_figure_means_no_bar() {
        let t = day_only(46 * 60, None);
        assert!(matches!(rows_head(&t), MenuRow::Header { bar: None, .. }));
    }

    /// No limit at all: a bar would imply a wall that isn't there.
    #[test]
    fn no_limit_means_no_bar() {
        let t = day_only(-1, Some(74 * 60));
        assert!(matches!(rows_head(&t), MenuRow::Header { bar: None, .. }));
    }

    fn rows_head(t: &TimeLeftView) -> MenuRow {
        menu_rows(Some(t))[0].clone()
    }

    /// The header already said "46m left today" — repeating it as a detail
    /// line is the sort of thing that makes a menu feel machine-written.
    #[test]
    fn the_detail_never_repeats_the_headline() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.schedule_seconds = 2 * 3600 + 35 * 60;
        let ls = labels(&menu_rows(Some(&t)));
        assert_eq!(
            ls.iter().filter(|l| l.contains("left today")).count(),
            1,
            "the day is said once, not twice: {ls:?}"
        );
        assert!(
            ls.iter().any(|l| l.contains("until the day's hours end")),
            "the window is still its own line: {ls:?}"
        );
    }

    /// Each named allowance is its own row with its own meter.
    #[test]
    fn buckets_get_a_row_each_with_their_own_meter() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![
            BucketView {
                id: "play".into(),
                label: "Play".into(),
                used_seconds: 35 * 60,
                limit_seconds: 60 * 60,
                remaining_seconds: 25 * 60,
                week_limit_seconds: -1,
                week_remaining_seconds: -1,
                spent: false,
                capped: true,
            },
            BucketView {
                id: "school".into(),
                label: "School".into(),
                used_seconds: 40 * 60,
                limit_seconds: 0,
                remaining_seconds: 0,
                week_limit_seconds: -1,
                week_remaining_seconds: -1,
                spent: false,
                capped: false,
            },
        ];
        let rows = menu_rows(Some(&t));
        let play = rows
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { label, detail, bar } if label == "Play" => {
                    Some((detail.clone(), *bar))
                }
                _ => None,
            })
            .expect("Play has a row");
        assert_eq!(play.0, "25m left");
        assert!((play.1.expect("capped bucket draws a meter") - 35.0 / 60.0).abs() < 0.001);

        // A lifted bucket still shows — the meter runs, nothing is being taken
        // — but there is no wall to draw a bar against.
        let school = rows
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { label, detail, bar } if label == "School" => {
                    Some((detail.clone(), *bar))
                }
                _ => None,
            })
            .expect("School has a row");
        assert_eq!(school.1, None);
        assert!(
            !school.0.contains("left"),
            "no wall, no countdown: {school:?}"
        );
    }

    /// A spent bucket says so plainly rather than counting down from zero.
    #[test]
    fn a_spent_bucket_says_it_is_used_up() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "play".into(),
            label: "Play".into(),
            used_seconds: 60 * 60,
            limit_seconds: 60 * 60,
            remaining_seconds: 0,
            week_limit_seconds: -1,
            week_remaining_seconds: -1,
            spent: true,
            capped: true,
        }];
        let d = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(d, "used up");
    }

    /// A group on BOTH a day and a week cap says both, in one breath — the
    /// week part only when a weekly cap is actually set (`!= -1`).
    #[test]
    fn a_bucket_on_a_day_and_week_cap_says_both() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "play".into(),
            label: "Play".into(),
            used_seconds: 15 * 60,
            limit_seconds: 60 * 60,
            remaining_seconds: 45 * 60,
            week_limit_seconds: 5 * 3600,
            week_remaining_seconds: 2 * 3600 + 10 * 60,
            spent: false,
            capped: true,
        }];
        let d = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(d, "45m left today · 2h 10m left this week");
    }

    /// A group with no day cap of its own (weekly-only) has its day slot
    /// filled from the week's own numbers by the daemon, so the two read
    /// identical — the row says the one true sentence, not the same figure
    /// twice.
    #[test]
    fn a_weekly_only_bucket_row_reads_correctly() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "reading".into(),
            label: "Reading".into(),
            used_seconds: 30 * 60,
            limit_seconds: 3 * 3600, // filled from the week limit (no day cap)
            remaining_seconds: 2 * 3600 + 30 * 60,
            week_limit_seconds: 3 * 3600,
            week_remaining_seconds: 2 * 3600 + 30 * 60,
            spent: false,
            capped: true,
        }];
        let d = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(d, "2h 30m left this week");
        assert!(!d.contains("today"), "no day wall to say twice: {d}");
    }

    /// F3: a weekly-only bucket's bar must be driven off the WEEK meter, not
    /// today's day meter — before this fix, a ward barely started for the day
    /// but 4 of 5 hours into their week read as a nearly-empty bar (`used_
    /// seconds` — today only — divided by `limit_seconds`, which for a
    /// weekly-only bucket IS the week wall).
    #[test]
    fn a_weekly_only_bucket_bar_reads_off_the_week_meter_not_today() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "reading".into(),
            label: "Reading".into(),
            // Today has barely started…
            used_seconds: 5 * 60,
            limit_seconds: 5 * 3600, // filled from the week limit (no day cap)
            remaining_seconds: 60 * 60,
            // …but the week is 4 of 5 hours gone.
            week_limit_seconds: 5 * 3600,
            week_remaining_seconds: 60 * 60,
            spent: false,
            capped: true,
        }];
        let bar = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { label, bar, .. } if label == "Reading" => Some(*bar),
                _ => None,
            })
            .unwrap();
        let b = bar.expect("a capped weekly-only bucket still draws a bar");
        assert!(
            (b - 4.0 / 5.0).abs() < 0.001,
            "bar must read the week's 4-of-5, not today's 5 minutes of 5 hours: {b}"
        );
    }

    /// A day-capped bucket (no week cap at all) is unaffected by the F3 fix —
    /// its bar is still today's meter over today's own wall.
    #[test]
    fn a_day_only_bucket_bar_is_unchanged_by_the_weekly_only_fix() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "play".into(),
            label: "Play".into(),
            used_seconds: 35 * 60,
            limit_seconds: 60 * 60,
            remaining_seconds: 25 * 60,
            week_limit_seconds: -1,
            week_remaining_seconds: -1,
            spent: false,
            capped: true,
        }];
        let bar = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { label, bar, .. } if label == "Play" => Some(*bar),
                _ => None,
            })
            .unwrap();
        let b = bar.expect("a capped day bucket still draws a bar");
        assert!((b - 35.0 / 60.0).abs() < 0.001, "bar was {b}");
    }

    /// A PAUSED set draws no bar at all, regardless of day/weekly shape — same
    /// as before the F3 fix.
    #[test]
    fn a_paused_bucket_draws_no_bar_weekly_or_daily() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![BucketView {
            id: "reading".into(),
            label: "Reading".into(),
            used_seconds: 5 * 60,
            limit_seconds: 0,
            remaining_seconds: 0,
            week_limit_seconds: -1,
            week_remaining_seconds: -1,
            spent: false,
            capped: false,
        }];
        let bar = menu_rows(Some(&t))
            .iter()
            .find_map(|r| match r {
                MenuRow::Bucket { label, bar, .. } if label == "Reading" => Some(*bar),
                _ => None,
            })
            .unwrap();
        assert_eq!(bar, None, "a lifted bucket has no wall to draw against");
    }

    /// A SPENT group offers its own "ask for more" — right params, right
    /// copy — and a group that still has time, or is lifted, offers no such
    /// row at all: there is nothing to ask for yet.
    #[test]
    fn a_spent_group_offers_ask_for_more_and_a_healthy_one_does_not() {
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.buckets = vec![
            BucketView {
                id: "play".into(),
                label: "Play".into(),
                used_seconds: 60 * 60,
                limit_seconds: 60 * 60,
                remaining_seconds: 0,
                week_limit_seconds: -1,
                week_remaining_seconds: -1,
                spent: true,
                capped: true,
            },
            BucketView {
                id: "reading".into(),
                label: "Reading".into(),
                used_seconds: 10 * 60,
                limit_seconds: 60 * 60,
                remaining_seconds: 50 * 60,
                week_limit_seconds: -1,
                week_remaining_seconds: -1,
                spent: false,
                capped: true,
            },
        ];
        let rows = menu_rows(Some(&t));
        let asks: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                MenuRow::AskBucket {
                    bucket_id,
                    minutes,
                    label,
                    ..
                } => Some((bucket_id.clone(), *minutes, label.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            asks,
            vec![(
                "play".to_string(),
                BUCKET_ASK_MINUTES,
                "Ask for more Play time".to_string()
            )]
        );
    }

    /// The ward's `askFirst` apps render as an explicit invitation to ask —
    /// "Ask to open", never the flat "Blocked" wording a plain block gets.
    #[test]
    fn ask_first_apps_render_as_ask_to_open_not_blocked() {
        use charter_ipc::dto::AskFirstAppView;
        let mut t = day_only(46 * 60, Some(74 * 60));
        t.ask_first = vec![AskFirstAppView {
            pkg: "com.mojang.minecraftpe".into(),
            label: "Minecraft".into(),
        }];
        let rows = menu_rows(Some(&t));
        let row = rows
            .iter()
            .find_map(|r| match r {
                MenuRow::AskToOpen { pkg, label, .. } => Some((pkg.clone(), label.clone())),
                _ => None,
            })
            .expect("an askFirst app gets its own row");
        assert_eq!(row.0, "com.mojang.minecraftpe");
        assert_eq!(row.1, "Ask to open Minecraft");
        assert!(
            !labels(&rows).iter().any(|l| l.contains("Blocked")),
            "an ask-first app must never read as a flat block: {:?}",
            labels(&rows)
        );
    }

    /// No `askFirst` apps: no section at all, not an empty heading.
    #[test]
    fn no_ask_first_apps_means_no_ask_to_open_section() {
        let t = day_only(46 * 60, Some(74 * 60));
        let rows = menu_rows(Some(&t));
        assert!(!rows.iter().any(|r| matches!(r, MenuRow::AskToOpen { .. })));
    }

    /// The asks are offered, and the way into the full app is always last.
    #[test]
    fn the_asks_and_the_way_in_close_the_menu() {
        let t = day_only(46 * 60, Some(74 * 60));
        let rows = menu_rows(Some(&t));
        let asks: Vec<u16> = rows
            .iter()
            .filter_map(|r| match r {
                MenuRow::Ask {
                    minutes,
                    enabled: true,
                    ..
                } => Some(*minutes),
                _ => None,
            })
            .collect();
        assert_eq!(asks, vec![15, 30, 60]);
        assert!(matches!(rows.last(), Some(MenuRow::OpenApp { .. })));
    }

    /// An account Kintrinsic isn't watching is offered no asks — there is nobody
    /// to ask — but the way into the app stays, so a parent can still open it.
    #[test]
    fn an_unwatched_account_offers_no_asks_but_still_opens() {
        let rows = menu_rows(None);
        assert!(
            rows.iter()
                .all(|r| !matches!(r, MenuRow::Ask { enabled: true, .. })),
            "nothing to ask when the daemon is unreachable: {:?}",
            labels(&rows)
        );
        assert!(matches!(rows.last(), Some(MenuRow::OpenApp { .. })));
    }

    /// A locked ward can still ask — that is the whole point of the lock
    /// surface — and the menu says the way back, not the arithmetic.
    #[test]
    fn a_locked_ward_can_still_ask() {
        let mut t = day_only(0, Some(120 * 60));
        t.locked = true;
        t.reason = Some("budget".into());
        let rows = menu_rows(Some(&t));
        assert!(rows
            .iter()
            .any(|r| matches!(r, MenuRow::Ask { enabled: true, .. })));
        let ls = labels(&rows);
        assert!(ls[0].contains("Time's up"), "{ls:?}");
        assert!(
            ls.iter().any(|l| l.contains("starts again tomorrow")),
            "the way back is named: {ls:?}"
        );
    }

    // ---- which shell draws it -------------------------------------------

    /// Mint's desktops get the menu Mint draws.
    #[test]
    fn mints_desktops_take_the_xapp_shell() {
        for d in ["X-Cinnamon", "cinnamon", "MATE", "XFCE", "X-Cinnamon:GNOME"] {
            assert!(prefers_xapp_shell(d), "{d} should take the XApp shell");
        }
    }

    /// Everything else keeps the StatusNotifierItem shell.
    #[test]
    fn other_desktops_keep_todays_shell() {
        for d in ["GNOME", "KDE", "ubuntu:GNOME", "sway", ""] {
            assert!(!prefers_xapp_shell(d), "{d} should keep today's shell");
        }
    }
}
