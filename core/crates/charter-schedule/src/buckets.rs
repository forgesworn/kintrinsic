//! Named app buckets with a daily and/or weekly allowance — "Play is an hour a
//! day", "Play is five hours a week", or both at once.
//!
//! Two pure questions, both exhaustively tested here so the device-side wiring
//! carries no policy logic of its own:
//!
//! 1. [`bucket_for_app`] — which bucket does the app in the foreground belong
//!    to? (The tick loop credits that bucket's meters.)
//! 2. [`spent_bucket_apps`] — whose allowance is gone, so which apps must stop?
//!
//! The design distinction that matters: spending a bucket closes THAT BUCKET,
//! never the device. The ward keeps the rest of their day — homework, messages,
//! the thing they were half-way through writing — and only the bucket's own
//! apps stop.
//!
//! A bucket may cap the day, the week, or both; whichever set axis runs out
//! FIRST closes the bucket — the two are not summed or averaged, each is
//! independently binding.

use crate::clause::{AppBucket, GrantBuckets};

/// Which bucket owns this app identity, if any. `None` = ordinary screen time.
///
/// Matching is exact on the identity string, which is the same vocabulary
/// `appRules.pkg` uses (Android package id, or Linux exec path / flatpak id).
/// The *fuzzy* part — is this running process an instance of that identity —
/// is the enforcer's job (`pkg_matches_process`), deliberately not repeated
/// here.
pub fn bucket_for_app<'a>(body: &'a GrantBuckets, app: &str) -> Option<&'a AppBucket> {
    if body.is_paused() || !body.is_valid() {
        return None;
    }
    body.buckets
        .iter()
        .find(|b| b.apps.iter().any(|a| a == app))
}

/// A bucket's usage-to-date on both axes, as looked up by the device's
/// day-keyed and week-keyed meters. The week axis is meaningless (and
/// ignored) for a bucket with no `weekly_minutes` set, but the caller still
/// supplies both so one lookup closure covers every bucket regardless of
/// which axes it caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BucketSpent {
    pub day_secs: u64,
    pub week_secs: u64,
}

/// The app identities that must NOT run now, because ANY axis their bucket
/// has set (daily and/or weekly) is spent — the two are independently
/// binding, never summed.
///
/// Fail-safe: a paused or malformed clause blocks NOTHING. A cap that cannot be
/// trusted must not confiscate anything.
pub fn spent_bucket_apps<F>(body: &GrantBuckets, spent: F) -> Vec<String>
where
    F: Fn(&str) -> BucketSpent,
{
    if body.is_paused() || !body.is_valid() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for b in &body.buckets {
        let s = spent(&b.id);
        let day_gone = b
            .daily_minutes
            .is_some_and(|m| s.day_secs >= u64::from(m) * 60);
        let week_gone = b
            .weekly_minutes
            .is_some_and(|m| s.week_secs >= u64::from(m) * 60);
        if day_gone || week_gone {
            out.extend(b.apps.iter().cloned());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Seconds left today against the daily cap (saturating at zero); `None` when
/// no daily cap is set on this bucket.
pub fn bucket_day_remaining_secs(b: &AppBucket, day_secs: u64) -> Option<u64> {
    b.daily_minutes
        .map(|m| (u64::from(m) * 60).saturating_sub(day_secs))
}

/// Seconds left this week against the weekly cap (saturating at zero); `None`
/// when no weekly cap is set on this bucket.
pub fn bucket_week_remaining_secs(b: &AppBucket, week_secs: u64) -> Option<u64> {
    b.weekly_minutes
        .map(|m| (u64::from(m) * 60).saturating_sub(week_secs))
}

/// The bucket's binding remaining time — the MIN of whichever axes are set —
/// for the ward's own "45m of 60m" surface and the guardian's STATUS. A
/// bucket with both caps set is only as generous as its tightest axis.
pub fn bucket_remaining_secs(b: &AppBucket, spent: BucketSpent) -> u64 {
    bucket_day_remaining_secs(b, spent.day_secs)
        .into_iter()
        .chain(bucket_week_remaining_secs(b, spent.week_secs))
        .min()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(minutes: u16) -> GrantBuckets {
        GrantBuckets {
            v: 1,
            buckets: vec![AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["com.mojang.Minecraft".into(), "/usr/games/supertux2".into()],
                daily_minutes: Some(minutes),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        }
    }

    fn play_weekly(daily: Option<u16>, weekly: Option<u16>) -> GrantBuckets {
        GrantBuckets {
            v: if daily.is_none() { 2 } else { 1 },
            buckets: vec![AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["com.mojang.Minecraft".into()],
                daily_minutes: daily,
                weekly_minutes: weekly,
            }],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        }
    }

    #[test]
    fn weekly_cap_bites_even_with_daily_headroom() {
        let b = play_weekly(Some(60), Some(300));
        // 20m today but the week is spent: the group closes.
        let spent = |_: &str| BucketSpent {
            day_secs: 20 * 60,
            week_secs: 300 * 60,
        };
        assert_eq!(spent_bucket_apps(&b, spent), vec!["com.mojang.Minecraft"]);
    }

    #[test]
    fn weekly_only_group_is_valid_at_v2_and_enforced() {
        let b = play_weekly(None, Some(300));
        assert!(b.is_valid());
        assert!(spent_bucket_apps(&b, |_| BucketSpent {
            day_secs: 0,
            week_secs: 299 * 60
        })
        .is_empty());
        assert_eq!(
            spent_bucket_apps(&b, |_| BucketSpent {
                day_secs: 0,
                week_secs: 300 * 60
            })
            .len(),
            1
        );
    }

    #[test]
    fn a_capless_bucket_is_invalid() {
        assert!(!play_weekly(None, None).is_valid());
    }

    #[test]
    fn v1_wire_from_old_mycharter_still_reads() {
        // The pinned pre-weekly bytes must keep working verbatim.
        let body: GrantBuckets = serde_json::from_str(
            r#"{"v":1,"buckets":[{"id":"play","label":"Play","apps":["mc"],"dailyMinutes":60}],"tz":"Europe/London","issuedAt":1700}"#,
        ).unwrap();
        assert!(body.is_valid());
        assert_eq!(body.buckets[0].daily_minutes, Some(60));
        assert_eq!(body.buckets[0].weekly_minutes, None);
    }

    #[test]
    fn binding_remaining_is_the_min_axis() {
        let b = play_weekly(Some(60), Some(300));
        let bucket = &b.buckets[0];
        assert_eq!(bucket_day_remaining_secs(bucket, 15 * 60), Some(45 * 60));
        assert_eq!(bucket_week_remaining_secs(bucket, 290 * 60), Some(10 * 60));
        assert_eq!(
            bucket_remaining_secs(
                bucket,
                BucketSpent {
                    day_secs: 15 * 60,
                    week_secs: 290 * 60
                }
            ),
            10 * 60
        );
    }

    #[test]
    fn attributes_an_app_to_its_bucket() {
        let b = play(60);
        assert_eq!(
            bucket_for_app(&b, "com.mojang.Minecraft").unwrap().id,
            "play"
        );
        assert_eq!(
            bucket_for_app(&b, "/usr/games/supertux2").unwrap().id,
            "play"
        );
        assert!(bucket_for_app(&b, "/usr/bin/libreoffice").is_none());
    }

    /// Daily-only `BucketSpent` shorthand for tests that don't care about the
    /// weekly axis.
    fn day(secs: u64) -> BucketSpent {
        BucketSpent {
            day_secs: secs,
            week_secs: 0,
        }
    }

    #[test]
    fn nothing_is_blocked_while_the_allowance_holds() {
        let b = play(60);
        assert!(spent_bucket_apps(&b, |_| day(0)).is_empty());
        assert!(spent_bucket_apps(&b, |_| day(59 * 60)).is_empty());
    }

    /// The whole point: at the cap, every app in that bucket stops.
    #[test]
    fn every_app_in_a_spent_bucket_stops() {
        let b = play(60);
        let blocked = spent_bucket_apps(&b, |_| day(60 * 60));
        assert_eq!(
            blocked,
            vec!["/usr/games/supertux2", "com.mojang.Minecraft"]
        );
        // And stays stopped past the cap.
        assert_eq!(spent_bucket_apps(&b, |_| day(90 * 60)).len(), 2);
    }

    /// Swapping Minecraft for another game in the same bucket must not buy
    /// more time — that is exactly why this is a bucket and not a per-app cap.
    #[test]
    fn one_allowance_covers_every_app_in_the_bucket() {
        let b = play(60);
        // 40 min of Minecraft + 20 of SuperTux spends the SAME meter.
        let spent = 40 * 60 + 20 * 60;
        assert_eq!(spent_bucket_apps(&b, |_| day(spent)).len(), 2);
    }

    #[test]
    fn buckets_are_independent_of_one_another() {
        let body = GrantBuckets {
            v: 1,
            buckets: vec![
                AppBucket {
                    id: "play".into(),
                    label: "Play".into(),
                    apps: vec!["mc".into()],
                    daily_minutes: Some(60),
                    weekly_minutes: None,
                },
                AppBucket {
                    id: "social".into(),
                    label: "Social".into(),
                    apps: vec!["chat".into()],
                    daily_minutes: Some(30),
                    weekly_minutes: None,
                },
            ],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        };
        // Play spent, Social untouched.
        let blocked = spent_bucket_apps(&body, |id| day(if id == "play" { 3600 } else { 0 }));
        assert_eq!(blocked, vec!["mc"]);
    }

    /// A cap you cannot trust must never confiscate anything.
    #[test]
    fn a_paused_or_malformed_clause_blocks_nothing() {
        let mut paused = play(60);
        paused.paused = Some(true);
        assert!(spent_bucket_apps(&paused, |_| day(99 * 3600)).is_empty());
        assert!(bucket_for_app(&paused, "com.mojang.Minecraft").is_none());

        // Zero-minute "allowance" would be a permanent block wearing a cap's
        // clothes — rejected outright rather than enforced.
        let zero = play(0);
        assert!(!zero.is_valid());
        assert!(spent_bucket_apps(&zero, |_| day(0)).is_empty());

        let mut no_apps = play(60);
        no_apps.buckets[0].apps.clear();
        assert!(!no_apps.is_valid());
        assert!(spent_bucket_apps(&no_apps, |_| day(99 * 3600)).is_empty());

        let mut bad_id = play(60);
        bad_id.buckets[0].id = "Play Time!".into();
        assert!(!bad_id.is_valid());

        let mut dupe = play(60);
        dupe.buckets.push(dupe.buckets[0].clone());
        assert!(!dupe.is_valid());

        let mut wrong_v = play(60);
        wrong_v.v = 3;
        assert!(!wrong_v.is_valid());
    }

    #[test]
    fn remaining_counts_down_and_floors_at_zero() {
        let b = play(60);
        let bucket = &b.buckets[0];
        assert_eq!(bucket_remaining_secs(bucket, day(0)), 3600);
        assert_eq!(bucket_remaining_secs(bucket, day(15 * 60)), 45 * 60);
        assert_eq!(bucket_remaining_secs(bucket, day(3600)), 0);
        // Overshoot (a long final tick) must not wrap around into a huge number.
        assert_eq!(bucket_remaining_secs(bucket, day(99 * 3600)), 0);
    }

    #[test]
    fn an_empty_clause_is_inert() {
        let empty = GrantBuckets {
            v: 1,
            buckets: vec![],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        };
        assert!(empty.is_valid());
        assert!(spent_bucket_apps(&empty, |_| day(99 * 3600)).is_empty());
        assert!(bucket_for_app(&empty, "anything").is_none());
    }
}

/// Cross-stack agreement: these are the EXACT bytes MyCharter's
/// `bucketsToGrant` produces (pinned in `apps/charter-app/src/wire/
/// bucketsClause.test.ts`). If either side drifts, one of these two suites
/// fails — a clause the guardian can author but the device can't read would
/// otherwise look like "the limit just doesn't work".
#[cfg(test)]
mod wire_agreement {
    use super::*;
    use crate::clause::WeekStart;

    const FROM_MYCHARTER: &str = r#"{"v":1,"buckets":[{"id":"play","label":"Play","apps":["com.mojang.Minecraft","/usr/games/supertux2"],"dailyMinutes":60}],"tz":"Europe/London","issuedAt":1700}"#;

    #[test]
    fn the_device_reads_what_mycharter_writes() {
        let body: GrantBuckets = serde_json::from_str(FROM_MYCHARTER).unwrap();
        assert!(body.is_valid());
        assert_eq!(body.buckets.len(), 1);
        let b = &body.buckets[0];
        assert_eq!(b.id, "play");
        assert_eq!(b.label, "Play");
        assert_eq!(b.daily_minutes, Some(60));
        assert_eq!(b.weekly_minutes, None);
        assert_eq!(b.apps, vec!["com.mojang.Minecraft", "/usr/games/supertux2"]);
        assert_eq!(body.tz, "Europe/London");
        assert_eq!(body.issued_at, 1700);

        // …and it actually bites at the hour.
        let day = |secs: u64| BucketSpent {
            day_secs: secs,
            week_secs: 0,
        };
        assert!(spent_bucket_apps(&body, |_| day(59 * 60)).is_empty());
        assert_eq!(spent_bucket_apps(&body, |_| day(60 * 60)).len(), 2);
    }

    #[test]
    fn a_paused_clause_from_mycharter_caps_nothing() {
        let paused = FROM_MYCHARTER.replace(r#""v":1"#, r#""v":1,"paused":true"#);
        let body: GrantBuckets = serde_json::from_str(&paused).unwrap();
        assert!(body.is_paused());
        assert!(spent_bucket_apps(&body, |_| BucketSpent {
            day_secs: 99 * 3600,
            week_secs: 0
        })
        .is_empty());
    }

    /// THE VERSION RULE, v2 twin: the EXACT bytes MyCharter's `bucketsToGrant`
    /// emits for a weekly-only bucket set (pinned in
    /// `apps/charter-app/src/wire/bucketsClause.test.ts`, "contract.md's
    /// weekly-only worked example round-trips exactly" — also
    /// `spec/contract.md`'s "Time buckets" worked example). MyCharter emits
    /// `v: 2` the moment ANY bucket in the set is weekly-only; the device must
    /// read it, validate it, and enforce the weekly wall.
    const FROM_MYCHARTER_V2_WEEKLY: &str = r#"{"v":2,"buckets":[{"id":"play","label":"Play","apps":["com.mojang.minecraftpe"],"weeklyMinutes":300}],"tz":"Europe/London","issuedAt":1732550400,"weekStart":"mon"}"#;

    #[test]
    fn the_device_reads_the_v2_weekly_only_bytes_mycharter_writes() {
        let body: GrantBuckets = serde_json::from_str(FROM_MYCHARTER_V2_WEEKLY).unwrap();
        assert!(body.is_valid());
        assert_eq!(body.v, 2);
        assert_eq!(body.buckets.len(), 1);
        let b = &body.buckets[0];
        assert_eq!(b.id, "play");
        assert_eq!(b.label, "Play");
        assert_eq!(b.daily_minutes, None);
        assert_eq!(b.weekly_minutes, Some(300));
        assert_eq!(b.apps, vec!["com.mojang.minecraftpe"]);
        assert_eq!(body.tz, "Europe/London");
        assert_eq!(body.issued_at, 1732550400);
        assert_eq!(body.week_start, Some(WeekStart::Mon));

        // …and it actually bites at the week's 300th minute, with NO daily
        // wall at all (day usage alone never closes a weekly-only bucket).
        let spent = |day_secs: u64, week_secs: u64| BucketSpent {
            day_secs,
            week_secs,
        };
        assert!(spent_bucket_apps(&body, |_| spent(999 * 3600, 299 * 60)).is_empty());
        assert_eq!(spent_bucket_apps(&body, |_| spent(0, 300 * 60)).len(), 1);
    }

    #[test]
    fn a_paused_v2_clause_from_mycharter_caps_nothing() {
        let paused = FROM_MYCHARTER_V2_WEEKLY.replace(r#""v":2"#, r#""v":2,"paused":true"#);
        let body: GrantBuckets = serde_json::from_str(&paused).unwrap();
        assert!(body.is_paused());
        assert!(spent_bucket_apps(&body, |_| BucketSpent {
            day_secs: 0,
            week_secs: 99 * 3600
        })
        .is_empty());
    }
}
