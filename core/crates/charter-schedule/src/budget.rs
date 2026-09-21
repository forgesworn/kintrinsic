//! Budget (quota) evaluation. `quota_left = min over present caps of
//! (cap − used)`, saturating ≥0; absent caps unconstrained; `paused` ⇒ 0;
//! `revoked` ⇒ no constraint.

use crate::clause::GrantBudget;
use crate::minutes::MinuteSet;
use crate::usage::UsageLedger;

/// Remaining quota in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaStatus {
    Unbounded,
    Remaining(u64),
}

/// The freshest verified cross-device usage view for one child (from a
/// guardian-signed USAGE_SYNC, kind 31115) — the pooled-budget input. Built
/// from the stored payload by `charter_spine::usage_pool`; period keys are
/// compared against the ledger's CURRENT local keys, so a view from a rolled
/// day/week contributes zero (persist-last-known, never carried forward).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsolidatedUsage {
    pub day_key: String,
    pub spent_elsewhere_today_secs: u64,
    pub week_key: Option<String>,
    pub spent_elsewhere_week_secs: Option<u64>,
    /// Union-rule extension: the union of the OTHER devices' active minutes.
    pub elsewhere_minutes_today: Option<MinuteSet>,
}

/// The pooled `(today, week)` used-seconds for the quota computation.
///
/// Daily, when the view's `day_key` matches the ledger's current local day:
/// - **Union path** (both sides have minute journals): `|own ∪ elsewhere| * 60`
///   — simultaneous use across devices counts once.
/// - **Scalar fallback**: `local + spentElsewhereTodaySecs`. Also used when the
///   local ledger has seconds but an empty journal (a pre-B3 snapshot restored
///   mid-day) — the union would undercount the unjournaled local time.
///
/// Weekly is always scalar, gated on `week_key`. Absent/stale views only ever
/// UNDER-count the pool — a flaky relay can never cause a wrongful early lock.
fn pooled_used(
    usage: &UsageLedger,
    consolidated: Option<&ConsolidatedUsage>,
    now_unix: i64,
) -> (u64, u64) {
    let local_today = usage.used_today(now_unix);
    let local_week = usage.used_week(now_unix);
    let Some(c) = consolidated else {
        return (local_today, local_week);
    };

    let today = if c.day_key == usage.current_day_key(now_unix) {
        let own = usage.minutes_today(now_unix);
        match &c.elsewhere_minutes_today {
            Some(elsewhere) if !own.is_empty() || local_today == 0 => {
                u64::from(own.union(elsewhere).count()) * 60
            }
            _ => local_today.saturating_add(c.spent_elsewhere_today_secs),
        }
    } else {
        local_today
    };

    let week = match (&c.week_key, c.spent_elsewhere_week_secs) {
        (Some(wk), Some(elsewhere)) if *wk == usage.current_week_key(now_unix) => {
            local_week.saturating_add(elsewhere)
        }
        _ => local_week,
    };

    (today, week)
}

/// Compute the remaining quota for `budget` given `usage` at `now`.
pub fn quota_left(usage: &UsageLedger, budget: &GrantBudget, now_unix: i64) -> QuotaStatus {
    quota_left_pooled(usage, budget, None, now_unix)
}

/// [`quota_left`] with the cross-device pool: caps are drawn down by the POOLED
/// usage (`cap − pooled`), per the union rule when both minute journals exist.
/// Quota left with `extra_today_secs` added to TODAY's allowance before usage
/// is drawn down — the seam a granted extension needs.
///
/// Adding the grant *after* a saturating subtraction (the old shape) meant no
/// amount of further use could ever eat into it: `budget` sat at exactly the
/// granted figure until midnight and the ward was never re-locked, so every
/// "more time" grant was really "free for the rest of the day". Folding it
/// into the allowance makes granted minutes ordinary, spendable minutes.
///
/// Returns a SIGNED remainder so an overdrawn ward stays overdrawn: a grant
/// smaller than the deficit must not unlock them.
pub fn quota_left_signed_pooled(
    usage: &UsageLedger,
    budget: &GrantBudget,
    consolidated: Option<&ConsolidatedUsage>,
    now_unix: i64,
    // NET seconds moved on today's allowance: positive = the guardian gave
    // time, negative = the guardian took it back. Signed so the two cancel
    // exactly rather than depending on which arrived first.
    extra_today_secs: i64,
    // `baseline`: usage `(today, week)` when the day's first grant landed.
    baseline: Option<(u64, u64)>,
) -> Option<i64> {
    let (daily, weekly) = quota_parts_signed_pooled(
        usage,
        budget,
        consolidated,
        now_unix,
        extra_today_secs,
        baseline,
    );
    match (daily, weekly) {
        (None, None) => None,
        (d, w) => Some(match (d, w) {
            (Some(d), Some(w)) => d.min(w),
            (Some(d), None) => d,
            (None, Some(w)) => w,
            (None, None) => unreachable!(),
        }),
    }
}

/// The daily and weekly caps' remainders **separately**, before they are
/// collapsed to the binding one by [`quota_left_signed_pooled`].
///
/// A ward on both a daily and a weekly cap needs to be told which one is
/// running out — "40 minutes left today" and "2 hours left this week" are
/// different sentences with different consequences, and the minimum alone
/// cannot say which it is. `None` for a cap means that cap is not set (or the
/// whole budget is revoked); `Some(0)` on every SET cap means paused — and on
/// the daily slot alone when a paused budget sets neither.
///
/// Returns SIGNED remainders for the same reason as the collapsed form: an
/// overdrawn ward must stay overdrawn.
pub fn quota_parts_signed_pooled(
    usage: &UsageLedger,
    budget: &GrantBudget,
    consolidated: Option<&ConsolidatedUsage>,
    now_unix: i64,
    extra_today_secs: i64,
    baseline: Option<(u64, u64)>,
) -> (Option<i64>, Option<i64>) {
    if budget.revoked == Some(true) {
        return (None, None); // unbounded
    }
    if budget.paused == Some(true) {
        // Paused reads as "nothing left" on whichever caps are actually set,
        // so a paused budget can never be mistaken for an absent one — and on
        // the DAILY slot when NEITHER is set. `(None, None)` is how this
        // function says "unbounded", so a paused budget carrying no caps used
        // to enforce nothing at all, against the contract ("blocks all time
        // (quota = 0). Distinct from absent.") and against
        // `quota_left_pooled` below, which always answered zero.
        if budget.daily_minutes.is_none() && budget.weekly_minutes.is_none() {
            return (Some(0), None);
        }
        return (
            budget.daily_minutes.map(|_| 0),
            budget.weekly_minutes.map(|_| 0),
        );
    }
    let (pooled_today, pooled_week) = pooled_used(usage, consolidated, now_unix);
    let daily = budget.daily_minutes.map(|daily| {
        // The allowance is floored at wherever the debt stood when the grant
        // landed, so a grant to an already-overdrawn ward buys the minutes it
        // says rather than being eaten by the existing deficit. With no grant
        // (`baseline` None) this is exactly the plain allowance.
        let base = (daily as i64 * 60).max(baseline.map_or(i64::MIN, |(t, _)| t as i64));
        base + extra_today_secs - pooled_today as i64
    });
    let weekly = budget.weekly_minutes.map(|weekly| {
        // The grant lifts the week too: minutes given today are spent today,
        // and a weekly cap that ignored them would lock a ward the guardian
        // has just deliberately let on.
        let base = (weekly as i64 * 60).max(baseline.map_or(i64::MIN, |(_, w)| w as i64));
        base + extra_today_secs - pooled_week as i64
    });
    (daily, weekly)
}

pub fn quota_left_pooled(
    usage: &UsageLedger,
    budget: &GrantBudget,
    consolidated: Option<&ConsolidatedUsage>,
    now_unix: i64,
) -> QuotaStatus {
    if budget.revoked == Some(true) {
        return QuotaStatus::Unbounded;
    }
    if budget.paused == Some(true) {
        return QuotaStatus::Remaining(0);
    }
    let (pooled_today, pooled_week) = pooled_used(usage, consolidated, now_unix);
    let mut rem: Option<u64> = None;
    if let Some(daily) = budget.daily_minutes {
        let left = (daily as u64 * 60).saturating_sub(pooled_today);
        rem = Some(rem.map_or(left, |r| r.min(left)));
    }
    if let Some(weekly) = budget.weekly_minutes {
        let left = (weekly as u64 * 60).saturating_sub(pooled_week);
        rem = Some(rem.map_or(left, |r| r.min(left)));
    }
    match rem {
        None => QuotaStatus::Unbounded,
        Some(r) => QuotaStatus::Remaining(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::WeekStart;
    use crate::usage::{Activity, UsageLedger};

    const TZ: &str = "Europe/London";
    const NOON: i64 = 1_782_734_400; // Mon 2026-06-29 12:00 BST

    fn budget(daily: Option<u32>, weekly: Option<u32>) -> GrantBudget {
        GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: daily,
            weekly_minutes: weekly,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        }
    }

    #[test]
    fn at_daily_limit_locks() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 120 * 60); // 120 min used
        assert_eq!(
            quota_left(&u, &budget(Some(120), None), NOON),
            QuotaStatus::Remaining(0)
        );
    }

    #[test]
    fn min_of_daily_and_weekly() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 60 * 60); // 60 min
                                                   // daily 120 -> 60 left; weekly 90 -> 30 left; min = 30 min = 1800s.
        assert_eq!(
            quota_left(&u, &budget(Some(120), Some(90)), NOON),
            QuotaStatus::Remaining(1800)
        );
    }

    #[test]
    fn paused_is_zero_revoked_unconstrained() {
        let u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        let mut b = budget(Some(120), None);
        b.paused = Some(true);
        assert_eq!(quota_left(&u, &b, NOON), QuotaStatus::Remaining(0));
        b.paused = None;
        b.revoked = Some(true);
        assert_eq!(quota_left(&u, &b, NOON), QuotaStatus::Unbounded);
    }

    /// The enforcer's own function (`quota_parts_signed_pooled`) and the
    /// exported one (`quota_left`) are two implementations of one rule; on a
    /// paused budget with NO caps they used to disagree — zero vs unbounded —
    /// and the enforcer's answer, the one that matters, was the wrong one.
    #[test]
    fn a_paused_budget_with_no_caps_is_still_zero_for_the_enforcer() {
        let u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        let mut b = budget(None, None);
        b.paused = Some(true);
        assert_eq!(quota_left(&u, &b, NOON), QuotaStatus::Remaining(0));
        let (daily, weekly) = quota_parts_signed_pooled(&u, &b, None, NOON, 0, None);
        assert_eq!(
            (daily, weekly),
            (Some(0), None),
            "never (None, None) = unbounded"
        );
        // A grant does not reopen a paused budget.
        let (daily, _) = quota_parts_signed_pooled(&u, &b, None, NOON, 3600, None);
        assert_eq!(daily, Some(0));
    }

    #[test]
    fn absent_caps_unconstrained() {
        let u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        assert_eq!(
            quota_left(&u, &budget(None, None), NOON),
            QuotaStatus::Unbounded
        );
    }

    // ---- pooled (B3) -------------------------------------------------------

    fn minutes(range: std::ops::Range<usize>) -> MinuteSet {
        let mut m = MinuteSet::default();
        for i in range {
            m.set(i);
        }
        m
    }

    fn view(u: &UsageLedger) -> ConsolidatedUsage {
        ConsolidatedUsage {
            day_key: u.current_day_key(NOON),
            ..ConsolidatedUsage::default()
        }
    }

    #[test]
    fn pooled_scalar_locks_when_elsewhere_plus_local_exceeds_cap() {
        // cap 60m; 30m here + 35m elsewhere = 65m pooled -> locked.
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 30 * 60);
        let c = ConsolidatedUsage {
            spent_elsewhere_today_secs: 35 * 60,
            elsewhere_minutes_today: None,
            ..view(&u)
        };
        // The local journal is non-empty but the view has no bitmap: scalar path.
        assert_eq!(
            quota_left_pooled(&u, &budget(Some(60), None), Some(&c), NOON),
            QuotaStatus::Remaining(0)
        );
        // Without the view, only local counts.
        assert_eq!(
            quota_left(&u, &budget(Some(60), None), NOON),
            QuotaStatus::Remaining(30 * 60)
        );
    }

    #[test]
    fn pooled_union_counts_simultaneous_once() {
        // cap 60m. This device was active local minutes 720..760 (40m,
        // credited as one 40m span ending 13:40 local = minute 820... use
        // mark via credit at NOON+40m). Elsewhere covers an overlapping 30m.
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        // 40 minutes of active time ending at NOON+2400 -> local minutes 780..820.
        u.credit(NOON + 2400, Activity::Active, 2400);
        let own = u.minutes_today(NOON + 2400);
        assert_eq!(own.count(), 40);
        // Elsewhere: 30 minutes, 20 of them overlapping (local 800..830).
        let c = ConsolidatedUsage {
            day_key: u.current_day_key(NOON),
            spent_elsewhere_today_secs: 30 * 60, // scalar would say 40+30=70m -> lock
            elsewhere_minutes_today: Some(minutes(800..830)),
            ..ConsolidatedUsage::default()
        };
        // Union = 780..830 = 50 distinct minutes -> 10m left, NOT locked.
        assert_eq!(
            quota_left_pooled(&u, &budget(Some(60), None), Some(&c), NOON + 2400),
            QuotaStatus::Remaining(10 * 60)
        );
    }

    #[test]
    fn stale_day_key_falls_back_to_local_only() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 10 * 60);
        let c = ConsolidatedUsage {
            day_key: "2026-06-28".into(), // yesterday's view
            spent_elsewhere_today_secs: 55 * 60,
            elsewhere_minutes_today: Some(minutes(0..55)),
            ..ConsolidatedUsage::default()
        };
        assert_eq!(
            quota_left_pooled(&u, &budget(Some(60), None), Some(&c), NOON),
            QuotaStatus::Remaining(50 * 60)
        );
    }

    #[test]
    fn pooled_week_scalar_gated_on_week_key() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 60 * 60);
        let mut c = ConsolidatedUsage {
            week_key: Some(u.current_week_key(NOON)),
            spent_elsewhere_week_secs: Some(4 * 3600),
            ..view(&u)
        };
        // weekly cap 6h: 1h here + 4h elsewhere -> 1h left.
        assert_eq!(
            quota_left_pooled(&u, &budget(None, Some(360)), Some(&c), NOON),
            QuotaStatus::Remaining(3600)
        );
        // A rolled week key contributes nothing.
        c.week_key = Some("2026-06-22".into());
        assert_eq!(
            quota_left_pooled(&u, &budget(None, Some(360)), Some(&c), NOON),
            QuotaStatus::Remaining(5 * 3600)
        );
    }

    #[test]
    fn no_view_matches_old_behavior_and_never_overcounts() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 45 * 60);
        assert_eq!(
            quota_left_pooled(&u, &budget(Some(60), None), None, NOON),
            quota_left(&u, &budget(Some(60), None), NOON)
        );
    }
}
