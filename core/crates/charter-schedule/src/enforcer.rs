//! The enforcer decision engine: `effective = min(window-left, quota-left)`,
//! each offset by its today-only extension pool (schedule capped at end-of-day
//! in tz). Edge-triggered Freeze/Thaw + ShowLock/HideLock + once-each warnings +
//! Audit + TimeLeft. **Every freeze targets `ManagedAppSlice`**; the guard
//! rejects freezing charterd or the lock UI. Enforcement is **fail-SAFE**: a
//! malformed/unparseable clause locks (never fail-open).

use chrono::{DateTime, Duration, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;

use crate::budget::ConsolidatedUsage;
use crate::clause::{GrantBudget, GrantSchedule};
use crate::extension::ExtensionLedger;
use crate::schedule_eval::{evaluate_grant_schedule, ScheduleStatus};
use crate::usage::UsageLedger;

/// The only legal freeze target: the managed child's whole user slice — never
/// charterd or the lock UI (root, in `system.slice`, outside the frozen subtree).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeTarget {
    ManagedAppSlice,
}

/// Runtime guard for the freeze target string the daemon maps to a cgroup path.
/// Accepts only a managed child's user slice (`user.slice/user-<uid>.slice`);
/// rejects the root user, system/charterd slices, and the lock scope. The uid is
/// roster-validated upstream — this is the defense-in-depth string guard.
pub fn is_valid_freeze_target(name: &str) -> bool {
    name.starts_with("user.slice/user-")
        && name.ends_with(".slice")
        && !name.starts_with("user.slice/user-0.slice")
        && !name.contains("charterd")
        && !name.contains("lock")
}

/// What to apply to a managed child's freeze slice on a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeAction {
    Freeze,
    Thaw,
}

/// Reconcile a child's freeze slice to their CURRENT locked state — **level**-
/// triggered, unlike the [`EnforcerCore`]'s edge-triggered `Freeze`/`Thaw`
/// *effects* (which fire once on the transition). The enforcement loop calls
/// this every tick so a freeze is never lost:
///
/// - The `Freeze` effect fires once when a child crosses into "locked". If the
///   apply fails (the child is not logged in yet, so their cgroup slice does not
///   exist) the edge is consumed and never re-emitted — the child would then run
///   unfrozen forever once they *do* log in. Reconciling on `locked` re-applies
///   the freeze as soon as the slice appears.
/// - `slice_exists == false` (no live session) yields `None`: there is nothing to
///   freeze, so the daemon does nothing rather than erroring on a missing cgroup.
///
/// Re-applying the same state is a kernel no-op, so calling this every tick is
/// safe (no freeze/thaw thrashing).
pub fn reconcile_freeze(locked: bool, slice_exists: bool) -> Option<FreezeAction> {
    if !slice_exists {
        None
    } else if locked {
        Some(FreezeAction::Freeze)
    } else {
        Some(FreezeAction::Thaw)
    }
}

/// Which boundary forced the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockReason {
    Schedule,
    Budget,
    /// A malformed/unparseable clause — fail-SAFE lock.
    Malformed,
    /// The guardian called a stand-down: "finish up now". Distinct from `Budget`
    /// because the ward's way out is different and she has to be told which —
    /// a spent budget waits for tomorrow, a closed window waits for the window,
    /// but a stand-down waits for a PERSON. A lock that cannot explain itself is
    /// the product being opaque at the worst possible moment.
    StandDown,
}

/// Warning thresholds (emitted once each, re-armed after a thaw).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarnLevel {
    Ten,
    One,
}

/// Audit classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    Locked,
    Thawed,
}

/// The full time-left breakdown (`-1` = unbounded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Remaining {
    pub effective_secs: i64,
    pub schedule_secs: i64,
    pub budget_secs: i64,
    pub extension_secs: i64,
    pub locked: bool,
    pub reason: Option<LockReason>,
    /// When schedule-locked, seconds until the next allowed window opens
    /// (`None` if not schedule-locked or no window within the lookahead). Lets
    /// the lock screen show "access resumes in …".
    pub next_open_secs: Option<i64>,
    /// The DAILY and WEEKLY caps' remainders separately (`-1` = that cap is not
    /// set). `budget_secs` above is whichever of the two binds, which cannot
    /// tell a ward on both caps *which* one is about to stop them — and "you
    /// have used this week up" needs a different plan from "come back
    /// tomorrow".
    pub budget_day_secs: i64,
    pub budget_week_secs: i64,
}

/// An effect the runtime carries out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnforcerEffect {
    Freeze(FreezeTarget),
    Thaw(FreezeTarget),
    ShowLock(LockReason),
    HideLock,
    Warn(WarnLevel),
    Audit(AuditKind),
    TimeLeft(Remaining),
}

/// Inputs to a tick. The clauses are the *authenticated, cached* clauses.
pub struct EnforcerInputs<'a> {
    pub now_unix: i64,
    pub schedule: Option<&'a GrantSchedule>,
    pub budget: Option<&'a GrantBudget>,
    pub usage: &'a UsageLedger,
    pub extension: &'a ExtensionLedger,
    /// The freshest verified cross-device usage view (USAGE_SYNC, B3), when
    /// any — the pooled-budget input. `None` = enforce on local usage only.
    pub consolidated: Option<&'a ConsolidatedUsage>,
    /// A live guardian stand-down, if one stands. `None` = none in force.
    pub stand_down: Option<StandDown>,
}

/// A stand-down in force, reduced to the only thing enforcement needs: how long
/// until it bites.
///
/// The grace clock lives on the DEVICE, not here — it pins when it first saw the
/// clause id and hands us the countdown — so this stays a pure function of its
/// inputs, with no second notion of "now" to disagree with `now_unix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandDown {
    /// Seconds until the lock lands. `0` = locked now.
    pub secs_until_lock: i64,
}

/// The timezone enforcement keys off — the schedule clause's tz, else the
/// budget clause's tz, else UTC. Day/week resets and end-of-day all derive from
/// this so the device and the guardian agree on what "today" is.
pub fn enforcement_tz_of(schedule: Option<&GrantSchedule>, budget: Option<&GrantBudget>) -> Tz {
    schedule
        .map(|s| s.tz.as_str())
        .or(budget.map(|b| b.tz.as_str()))
        .and_then(|s| s.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

fn enforcement_tz(inp: &EnforcerInputs) -> Tz {
    enforcement_tz_of(inp.schedule, inp.budget)
}

/// The unix instant of local midnight starting `date` in `tz`, robust across
/// DST: an ambiguous (fall-back) midnight uses the earlier instant; a midnight
/// skipped by a spring-forward gap uses the first valid instant after the gap.
/// Never silently falls back to "now".
fn local_midnight_ts(tz: Tz, date: chrono::NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("valid midnight");
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

fn secs_to_eod(tz: Tz, now_unix: i64) -> u64 {
    let now: DateTime<Tz> = DateTime::from_timestamp(now_unix, 0)
        .expect("valid timestamp")
        .with_timezone(&tz);
    // The end of the CURRENT service day is the next local midnight — derived
    // from the calendar date, NOT `now + 24h` (which lands on the wrong day on a
    // 23h/25h DST date and can read as the past, zeroing the window).
    let tomorrow = now.date_naive().succ_opt().expect("date in range");
    let midnight = local_midnight_ts(tz, tomorrow);
    (midnight - now_unix).max(0) as u64
}

/// The unix timestamp of the next local midnight in `tz` (end of the current
/// service day) — used by the time.extend enactor's today-only check.
pub fn end_of_day_unix(tz: &str, now_unix: i64) -> i64 {
    let parsed: Tz = tz.parse().unwrap_or(chrono_tz::UTC);
    now_unix + secs_to_eod(parsed, now_unix) as i64
}

/// End-of-day unix for the *clauses'* enforcement tz — the today-only cap the
/// time.extend enactor must use so it agrees with `compute_remaining` (never
/// the daemon's ambient runtime tz).
pub fn enforcement_eod_unix(
    schedule: Option<&GrantSchedule>,
    budget: Option<&GrantBudget>,
    now_unix: i64,
) -> i64 {
    now_unix + secs_to_eod(enforcement_tz_of(schedule, budget), now_unix) as i64
}

/// Minimum treating `-1` as unbounded (+∞).
fn min_unbounded(a: i64, b: i64) -> i64 {
    match (a < 0, b < 0) {
        (true, true) => -1,
        (true, false) => b,
        (false, true) => a,
        (false, false) => a.min(b),
    }
}

/// Burn wall-clock off the schedule extension pool when — and only when —
/// the schedule window is CLOSED: out-of-window the pool is the only thing
/// keeping the device unlocked, so it must count down; in-window it stays
/// whole ("+30 minutes" means 30 past the close). Every enforcing tick calls
/// this with its elapsed before computing remaining. No schedule = nothing
/// to burn against (the pool is inert anyway).
pub fn burn_schedule_extension(
    extension: &mut ExtensionLedger,
    schedule: Option<&GrantSchedule>,
    now_unix: i64,
    elapsed_secs: u64,
) {
    if elapsed_secs == 0 {
        return;
    }
    if let Some(s) = schedule {
        if matches!(
            evaluate_grant_schedule(s, now_unix),
            ScheduleStatus::Locked { .. }
        ) {
            extension.burn_schedule(now_unix, elapsed_secs);
        }
    }
}

/// Compute the time-left breakdown. Pure; fail-SAFE on malformed clauses.
pub fn compute_remaining(inp: &EnforcerInputs) -> Remaining {
    let tz = enforcement_tz(inp);
    let eod = secs_to_eod(tz, inp.now_unix);

    // Schedule dimension.
    let mut malformed = false;
    let mut next_open_secs: Option<i64> = None;
    let sched_base: i64 = match inp.schedule {
        None => -1,
        Some(s) => match evaluate_grant_schedule(s, inp.now_unix) {
            ScheduleStatus::Unbounded => -1,
            ScheduleStatus::Open { seconds_to_close } => seconds_to_close as i64,
            ScheduleStatus::Locked { seconds_to_open } => {
                next_open_secs = seconds_to_open.map(|s| s as i64);
                0
            }
            ScheduleStatus::Unparseable => {
                malformed = true;
                0
            }
        },
    };
    let sched_ext = inp.extension.schedule_extra_secs(inp.now_unix);
    let sched_deduct = inp.extension.schedule_deducted_secs(inp.now_unix);
    let schedule_secs: i64 = if malformed {
        // Fail-SAFE: a malformed/unparseable clause locks regardless of any
        // extension pool — an extension must never mask a clause we cannot
        // authenticate-and-evaluate.
        0
    } else if sched_base < 0 {
        // Nothing to take time OFF of: an unbounded window stays unbounded
        // rather than being turned into a finite one by a deduction. The
        // surface that offers the deduction says so instead of silently
        // no-op'ing (there is no allowance here to reduce).
        -1
    } else {
        // The extension cannot push past end-of-day (today-only); a deduction
        // saturates at zero rather than wrapping the window negative.
        ((sched_base as u64 + sched_ext)
            .saturating_sub(sched_deduct)
            .min(eod)) as i64
    };

    // Budget dimension — drawn down by the POOLED usage when a verified
    // cross-device view is present (B3), local-only otherwise.
    // The extension is folded into TODAY'S ALLOWANCE, not added on afterwards.
    // Added afterwards it sat on top of a saturating floor, so further use
    // could never consume it — a granted "5 more minutes" became free time
    // until midnight, and the ward was never re-locked (found on hardware
    // 2026-07-31). Folded in, granted minutes are ordinary spendable minutes.
    let budget_ext = inp.extension.budget_extra_secs(inp.now_unix);
    let budget_deduct = inp.extension.budget_deducted_secs(inp.now_unix);
    // The NET move to today's allowance: minutes given, less minutes taken
    // back. One signed number so a give and a take-back on the same day cancel
    // exactly, instead of the order they arrived in mattering.
    let budget_net = budget_ext as i64 - budget_deduct as i64;
    // Both caps separately, so a surface can say WHICH one is running out; the
    // binding figure below is still the only thing enforcement acts on.
    let (day_part, week_part) = match inp.budget {
        None => (None, None),
        Some(b) => crate::budget::quota_parts_signed_pooled(
            inp.usage,
            b,
            inp.consolidated,
            inp.now_unix,
            budget_net,
            inp.extension.budget_baseline_secs(inp.now_unix),
        ),
    };
    // Clamp only at the very end: an overdrawn ward stays locked, so a grant
    // smaller than the deficit cannot buy their way out.
    let budget_day_secs = day_part.map_or(-1, |d| d.max(0));
    let budget_week_secs = week_part.map_or(-1, |w| w.max(0));
    let budget_secs: i64 = match (day_part, week_part) {
        (None, None) => -1, // no cap set, or revoked -> unbounded
        (d, w) => match (d, w) {
            (Some(d), Some(w)) => d.min(w),
            (Some(d), None) => d,
            (None, Some(w)) => w,
            (None, None) => unreachable!(),
        }
        .max(0),
    };

    let charter_secs = min_unbounded(schedule_secs, budget_secs);

    // A stand-down CAPS whatever the charter would otherwise allow. Applied
    // AFTER both extension pools on purpose: that ordering is the whole reason
    // "Give time" cannot quietly undo a deliberate "finish up now" — precedence,
    // not a special case anybody has to remember. An unbounded charter (-1) is
    // capped too, so a ward with no clauses at all can still be stood down.
    let (effective, stood_down) = match inp.stand_down {
        Some(sd) => {
            let cap = sd.secs_until_lock.max(0);
            let capped = if charter_secs < 0 {
                cap
            } else {
                charter_secs.min(cap)
            };
            // "The stand-down is why she is locked" means the CAP reached zero —
            // not merely that one stands. A ward who runs out of budget during
            // her grace minute was stopped by the budget, and telling her to
            // wait for her guardian would be a lie she cannot act on.
            (capped, cap == 0)
        }
        None => (charter_secs, false),
    };

    let locked = effective == 0;
    let reason = if !locked {
        None
    } else if malformed {
        // Fail-safe still outranks everything: never dress an unreadable clause
        // up as a decision somebody made.
        Some(LockReason::Malformed)
    } else if stood_down {
        // The newest and most deliberate act wins the explanation, even if a
        // window or budget would also have closed: it is the one whose way out
        // is "your guardian", and the only one she could otherwise misread as
        // the phone being broken.
        Some(LockReason::StandDown)
    } else if schedule_secs == 0 {
        Some(LockReason::Schedule) // schedule-wins tiebreak
    } else {
        Some(LockReason::Budget)
    };

    Remaining {
        effective_secs: effective,
        schedule_secs,
        budget_secs,
        // NET across both dimensions: a surface showing "+30" while the
        // guardian had just taken 30 back would be reporting a pool that no
        // longer exists.
        extension_secs: (sched_ext + budget_ext) as i64 - (sched_deduct + budget_deduct) as i64,
        locked,
        reason,
        // Only meaningful when the schedule window (not budget) is what locked.
        next_open_secs: if reason == Some(LockReason::Schedule) {
            next_open_secs
        } else {
            None
        },
        budget_day_secs,
        budget_week_secs,
    }
}

/// The edge-triggered enforcer state machine.
#[derive(Debug, Default)]
pub struct EnforcerCore {
    locked: bool,
    warned_ten: bool,
    warned_one: bool,
}

impl EnforcerCore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Run one tick: compute the breakdown, emit edge-triggered freeze/thaw +
    /// warnings + audit + the time-left snapshot.
    pub fn tick(&mut self, inp: &EnforcerInputs) -> Vec<EnforcerEffect> {
        let rem = compute_remaining(inp);
        let mut effs = Vec::new();

        if rem.locked && !self.locked {
            self.locked = true;
            // ShowLock + grab happen before Freeze (topology handled by runtime).
            effs.push(EnforcerEffect::ShowLock(
                rem.reason.unwrap_or(LockReason::Schedule),
            ));
            effs.push(EnforcerEffect::Freeze(FreezeTarget::ManagedAppSlice));
            effs.push(EnforcerEffect::Audit(AuditKind::Locked));
        } else if !rem.locked && self.locked {
            self.locked = false;
            self.warned_ten = false;
            self.warned_one = false; // re-arm warnings after a thaw
            effs.push(EnforcerEffect::Thaw(FreezeTarget::ManagedAppSlice));
            effs.push(EnforcerEffect::HideLock);
            effs.push(EnforcerEffect::Audit(AuditKind::Thawed));
        }

        if !self.locked {
            // Re-arm on a rising edge: an extension, a day/week quota reset, OR a
            // transition to UNBOUNDED (effective < 0, e.g. a clause was removed)
            // lifts us back above a threshold without an intervening lock, so the
            // warning must be allowed to fire again. (Bug: gating re-arm on
            // `effective >= 0` left it armed-as-fired across an unbounded spell.)
            if rem.effective_secs < 0 || rem.effective_secs > 600 {
                self.warned_ten = false;
            }
            if rem.effective_secs < 0 || rem.effective_secs > 60 {
                self.warned_one = false;
            }
            // Never say "ten minutes" and "one minute" in the same breath. Any
            // sudden drop past both thresholds in a single tick arms both — a
            // stand-down capping an hour to 60s does it every time, and a
            // cross-device usage sync can too. The latch is still consumed so it
            // won't fire late; only the misleading message is withheld.
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

        effs.push(EnforcerEffect::TimeLeft(rem));
        effs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::{GrantScheduleWindow, WeekStart, WeeklySchedule};
    use crate::extension::Dimension;
    use crate::usage::{Activity, UsageLedger};

    const TZ: &str = "Europe/London";
    // Mon 2026-06-29 18:00 BST (17:00 UTC; inside the 16:00-20:00 window).
    const IN_WINDOW: i64 = 1_782_752_400;
    // Mon 2026-06-29 22:00 BST (21:00 UTC; after the window -> locked).
    const AFTER_WINDOW: i64 = 1_782_766_800;

    fn after_school() -> GrantSchedule {
        GrantSchedule {
            v: 1,
            tz: TZ.into(),
            paused: None,
            weekly: WeeklySchedule {
                mon: Some(vec![GrantScheduleWindow {
                    start: "16:00".into(),
                    end: "20:00".into(),
                }]),
                ..Default::default()
            },
            overrides: None,
            issued_at: 1,
        }
    }

    fn ledgers() -> (UsageLedger, ExtensionLedger) {
        (
            UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW),
            ExtensionLedger::new(TZ, IN_WINDOW),
        )
    }

    // ---- Taking time back ("you've had twenty minutes off today") ----------

    fn daily(minutes: u32) -> GrantBudget {
        GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(minutes),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        }
    }

    /// The headline case: 120-minute day, 30 used, take 20 back -> 70 left.
    /// The deduction moves the ALLOWANCE, never the usage record.
    #[test]
    fn a_budget_deduction_reduces_todays_allowance() {
        let (mut usage, mut ext) = ledgers();
        usage.credit(IN_WINDOW, Activity::Active, 30 * 60);
        ext.deduct(IN_WINDOW, "local-1", 20, Dimension::Budget);
        let b = daily(120);
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.budget_secs, 70 * 60);
        // The factual record of what they actually did is untouched.
        assert_eq!(usage.used_today(IN_WINDOW), 30 * 60);
    }

    /// Give and take-back of the same size cancel exactly, in either order.
    #[test]
    fn a_give_and_an_equal_take_back_cancel() {
        let b = daily(120);
        let mut give_first = ExtensionLedger::new(TZ, IN_WINDOW);
        give_first.apply(IN_WINDOW, "g", 30, Dimension::Budget);
        give_first.deduct(IN_WINDOW, "t", 30, Dimension::Budget);

        let mut take_first = ExtensionLedger::new(TZ, IN_WINDOW);
        take_first.deduct(IN_WINDOW, "t", 30, Dimension::Budget);
        take_first.apply(IN_WINDOW, "g", 30, Dimension::Budget);

        let usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        let of = |ext: &ExtensionLedger| {
            compute_remaining(&EnforcerInputs {
                stand_down: None,
                now_unix: IN_WINDOW,
                schedule: None,
                budget: Some(&b),
                usage: &usage,
                extension: ext,
                consolidated: None,
            })
            .budget_secs
        };
        assert_eq!(of(&give_first), 120 * 60);
        assert_eq!(of(&take_first), 120 * 60);
    }

    /// Taking back more than is left locks — and reads as an ordinary spent
    /// budget, because that is what it honestly is: the allowance is gone and
    /// the way out is tomorrow (or an ask), exactly as if they had used it.
    #[test]
    fn taking_back_more_than_remains_locks_on_budget() {
        let (mut usage, mut ext) = ledgers();
        usage.credit(IN_WINDOW, Activity::Active, 100 * 60);
        ext.deduct(IN_WINDOW, "local-1", 60, Dimension::Budget);
        let b = daily(120);
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(r.locked);
        assert_eq!(r.reason, Some(LockReason::Budget));
        assert_eq!(r.budget_secs, 0);
    }

    /// A deduction on the schedule dimension brings the window's close forward.
    #[test]
    fn a_schedule_deduction_brings_the_window_close_forward() {
        let (usage, mut ext) = ledgers();
        let s = after_school();
        let plain = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(plain.schedule_secs, 2 * 3600); // 18:00 -> 20:00
        ext.deduct(IN_WINDOW, "local-1", 30, Dimension::Schedule);
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.schedule_secs, 90 * 60);
    }

    /// Nothing to take FROM: an unbounded dimension stays unbounded rather than
    /// being quietly turned into a finite one. (The surface offering the
    /// deduction is what must say so — silently capping a ward who has no
    /// limit at all would be enforcement nobody asked for.)
    #[test]
    fn a_deduction_never_bounds_an_unbounded_charter() {
        let (usage, mut ext) = ledgers();
        ext.deduct(IN_WINDOW, "local-1", 30, Dimension::Schedule);
        ext.deduct(IN_WINDOW, "local-2", 30, Dimension::Budget);
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.schedule_secs, -1);
        assert_eq!(r.budget_secs, -1);
        assert!(!r.locked);
    }

    /// A take-back is today-only, exactly like a grant: tomorrow is clean.
    #[test]
    fn a_deduction_expires_at_the_day_roll() {
        let (usage, mut ext) = ledgers();
        ext.deduct(IN_WINDOW, "local-1", 60, Dimension::Budget);
        let b = daily(120);
        let tomorrow = IN_WINDOW + 24 * 3600;
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: tomorrow,
            schedule: None,
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.budget_secs, 120 * 60);
    }

    // ---- Which cap is which (day vs week) ----------------------------------

    fn day_and_week(daily: Option<u32>, weekly: Option<u32>) -> GrantBudget {
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

    fn with_budget(b: &GrantBudget, usage: &UsageLedger, ext: &ExtensionLedger) -> Remaining {
        compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: Some(b),
            usage,
            extension: ext,
            consolidated: None,
        })
    }

    /// The binding figure alone can't say WHICH cap is about to stop them, so
    /// both are reported. Here the week is the tighter one.
    #[test]
    fn the_day_and_the_week_are_reported_separately() {
        let (mut usage, ext) = ledgers();
        usage.credit(IN_WINDOW, Activity::Active, 30 * 60);
        let b = day_and_week(Some(120), Some(300));
        let r = with_budget(&b, &usage, &ext);
        assert_eq!(r.budget_day_secs, 90 * 60);
        assert_eq!(r.budget_week_secs, 270 * 60);
        // …and the enforced figure is still the tighter of the two.
        assert_eq!(r.budget_secs, 90 * 60);
    }

    /// A cap that is not set reads as `-1` ("no such wall"), never as zero —
    /// zero would tell a ward their week was gone when no weekly cap exists.
    #[test]
    fn an_absent_cap_reads_as_unset_not_as_used_up() {
        let (usage, ext) = ledgers();
        let r = with_budget(&day_and_week(Some(120), None), &usage, &ext);
        assert_eq!(r.budget_day_secs, 120 * 60);
        assert_eq!(r.budget_week_secs, -1);

        // No budget clause at all: neither wall exists.
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.budget_day_secs, -1);
        assert_eq!(r.budget_week_secs, -1);
    }

    /// A weekly cap can bind while the day still has room — the case the
    /// separate figures exist for.
    #[test]
    fn the_week_can_be_the_wall_while_the_day_has_room() {
        let (mut usage, ext) = ledgers();
        // 20 minutes today, but the week is nearly spent.
        usage.credit(IN_WINDOW, Activity::Active, 20 * 60);
        let b = day_and_week(Some(120), Some(25));
        let r = with_budget(&b, &usage, &ext);
        assert_eq!(r.budget_day_secs, 100 * 60, "plenty left today");
        assert_eq!(r.budget_week_secs, 5 * 60, "but the week is nearly gone");
        assert_eq!(r.budget_secs, 5 * 60, "the week is what binds");
        assert!(!r.locked);
    }

    /// A PAUSED budget reads as zero on the caps that exist and unset on the
    /// ones that don't, so "paused" can never be mistaken for "no limit".
    #[test]
    fn a_paused_budget_reads_as_nothing_left_on_the_caps_that_exist() {
        let (usage, ext) = ledgers();
        let mut b = day_and_week(Some(120), None);
        b.paused = Some(true);
        let r = with_budget(&b, &usage, &ext);
        assert_eq!(r.budget_day_secs, 0);
        assert_eq!(r.budget_week_secs, -1);
        assert!(r.locked);
    }

    /// A revoked budget is unbounded on both — nothing is being capped.
    #[test]
    fn a_revoked_budget_is_unbounded_on_both_caps() {
        let (usage, ext) = ledgers();
        let mut b = day_and_week(Some(120), Some(300));
        b.revoked = Some(true);
        let r = with_budget(&b, &usage, &ext);
        assert_eq!(r.budget_day_secs, -1);
        assert_eq!(r.budget_week_secs, -1);
        assert_eq!(r.budget_secs, -1);
    }

    /// An overdrawn ward's parts clamp at zero like the binding figure does,
    /// so no surface ever renders a negative "time left".
    #[test]
    fn an_overdrawn_ward_reports_zero_not_a_negative() {
        let (mut usage, ext) = ledgers();
        usage.credit(IN_WINDOW, Activity::Active, 200 * 60);
        let r = with_budget(&day_and_week(Some(120), Some(300)), &usage, &ext);
        assert_eq!(r.budget_day_secs, 0);
        assert_eq!(r.budget_week_secs, 100 * 60);
        assert!(r.locked);
    }

    // ---- Guardian stand-down ("finish up now") -----------------------------

    fn stand_down_at(secs: i64) -> Option<StandDown> {
        Some(StandDown {
            secs_until_lock: secs,
        })
    }

    /// The grace is what the ward is owed: capped to the warning, not locked yet.
    #[test]
    fn stand_down_caps_remaining_to_the_grace_and_does_not_lock_yet() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(60),
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.effective_secs, 60);
        assert!(!r.locked);
    }

    /// A ward with NO clauses at all is still stoppable — an unbounded charter
    /// (-1) must be capped, not left untouched.
    #[test]
    fn stand_down_caps_an_unbounded_charter() {
        let (usage, ext) = ledgers();
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(60),
            now_unix: IN_WINDOW,
            schedule: None,
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(r.effective_secs, 60);
        assert!(!r.locked);
    }

    /// Grace elapsed: locked, and attributed to the stand-down so the ward is
    /// told the way out is a person.
    #[test]
    fn stand_down_locks_when_the_grace_runs_out() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(0),
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(r.locked);
        assert_eq!(r.reason, Some(LockReason::StandDown));
        // Nothing to wait for: there is no window that reopens her.
        assert_eq!(r.next_open_secs, None);
    }

    /// THE decision this feature rests on: "Give time" must not quietly undo a
    /// deliberate "finish up now". The cap is applied after the extension pools,
    /// so a gift cannot outbid it.
    #[test]
    fn a_gift_cannot_lift_a_stand_down() {
        let (usage, mut ext) = ledgers();
        let s = after_school();
        ext.apply(
            IN_WINDOW,
            "gift-1",
            60,
            crate::extension::Dimension::Schedule,
        );
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(0),
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(r.locked, "an hour gifted must not reopen a stand-down");
        assert_eq!(r.reason, Some(LockReason::StandDown));
    }

    /// Lifting it restores exactly what the charter said — no residue.
    #[test]
    fn lifting_a_stand_down_restores_the_charter() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let inputs = |sd: Option<StandDown>| EnforcerInputs {
            stand_down: sd,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let under = compute_remaining(&inputs(stand_down_at(0)));
        let lifted = compute_remaining(&inputs(None));
        assert!(under.locked);
        assert!(!lifted.locked);
        assert_eq!(
            lifted.effective_secs,
            compute_remaining(&inputs(None)).effective_secs
        );
    }

    /// A ward who runs out of budget DURING her grace minute was stopped by the
    /// budget. Telling her to wait for her guardian would be a lie she cannot
    /// act on, so the stand-down must not claim an outcome it did not cause.
    #[test]
    fn a_budget_that_empties_during_the_grace_keeps_the_blame() {
        let (mut usage, ext) = ledgers();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(1),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        // Spend the whole minute.
        usage.credit(IN_WINDOW, Activity::Active, 60);
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(60), // still in grace
            now_unix: IN_WINDOW,
            schedule: None,
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(r.locked);
        assert_eq!(r.reason, Some(LockReason::Budget));
    }

    /// A malformed clause still outranks it: never dress an unreadable clause up
    /// as a decision somebody made.
    #[test]
    fn malformed_still_outranks_a_stand_down() {
        let (usage, ext) = ledgers();
        let mut bad = after_school();
        // An unparseable tz is what the evaluator calls malformed.
        bad.tz = "Not/ARealZone".into();
        let r = compute_remaining(&EnforcerInputs {
            stand_down: stand_down_at(0),
            now_unix: IN_WINDOW,
            schedule: Some(&bad),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(r.locked);
        assert_eq!(r.reason, Some(LockReason::Malformed));
    }

    /// Never "ten minutes left" and "one minute left" in the same breath. A
    /// stand-down drops an hour to 60s in one tick and arms both thresholds.
    #[test]
    fn stand_down_warns_once_not_twice() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let mut core = EnforcerCore::new();
        // Settle unlocked and well above both thresholds first.
        let effs = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(!effs.iter().any(|e| matches!(e, EnforcerEffect::Warn(_))));

        let effs = core.tick(&EnforcerInputs {
            stand_down: stand_down_at(60),
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        let warns: Vec<_> = effs
            .iter()
            .filter_map(|e| match e {
                EnforcerEffect::Warn(l) => Some(*l),
                _ => None,
            })
            .collect();
        assert_eq!(
            warns,
            vec![WarnLevel::One],
            "the ward must hear \"one minute\", never also \"ten minutes\""
        );
    }

    /// The on-device fail-open (2026-07-24): an approved out-of-window "+30m"
    /// froze at 30:00 and never re-locked. With the wall-clock burn, the
    /// countdown ticks down and the lock lands at expiry.
    #[test]
    fn out_of_window_extension_burns_down_and_relocks() {
        let (usage, mut ext) = ledgers();
        let s = after_school();
        ext.apply(
            AFTER_WINDOW,
            "req-x",
            30,
            crate::extension::Dimension::Schedule,
        );

        let remaining_at = |ext: &ExtensionLedger, now: i64| {
            compute_remaining(&EnforcerInputs {
                stand_down: None,
                now_unix: now,
                schedule: Some(&s),
                budget: None,
                usage: &usage,
                extension: ext,
                consolidated: None,
            })
        };

        // Freshly granted: unlocked with exactly the 30 minutes showing.
        let r0 = remaining_at(&ext, AFTER_WINDOW);
        assert!(!r0.locked);
        assert_eq!(r0.effective_secs, 30 * 60);

        // 10 minutes of 60s enforcement ticks: the countdown MOVES.
        for i in 1..=10 {
            burn_schedule_extension(&mut ext, Some(&s), AFTER_WINDOW + i * 60, 60);
        }
        let r10 = remaining_at(&ext, AFTER_WINDOW + 600);
        assert!(!r10.locked);
        assert_eq!(r10.effective_secs, 20 * 60, "must tick down, not freeze");

        // The remaining 20 minutes elapse: the lock lands.
        for i in 11..=30 {
            burn_schedule_extension(&mut ext, Some(&s), AFTER_WINDOW + i * 60, 60);
        }
        let r30 = remaining_at(&ext, AFTER_WINDOW + 30 * 60);
        assert!(r30.locked, "expired extension must re-lock");
        assert_eq!(r30.reason, Some(LockReason::Schedule));
    }

    #[test]
    fn in_window_extension_still_extends_past_close_unburned() {
        // Granted DURING the window: no burn happens in-window, so the +30m
        // still means 30 minutes past the close (the pre-fix behaviour that
        // must be preserved).
        let (usage, mut ext) = ledgers();
        let s = after_school();
        ext.apply(
            IN_WINDOW,
            "req-y",
            30,
            crate::extension::Dimension::Schedule,
        );
        // Ticks while the window is OPEN must not burn the pool.
        for i in 1..=10 {
            burn_schedule_extension(&mut ext, Some(&s), IN_WINDOW + i * 60, 60);
        }
        let r = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW + 600,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(!r.locked);
        assert_eq!(r.extension_secs, 30 * 60, "in-window pool stays whole");
    }

    #[test]
    fn reconcile_freeze_is_level_triggered_and_existence_gated() {
        // Locked child with a live session -> freeze; unlocked -> thaw.
        assert_eq!(reconcile_freeze(true, true), Some(FreezeAction::Freeze));
        assert_eq!(reconcile_freeze(false, true), Some(FreezeAction::Thaw));
        // Locked but not logged in (no slice) -> do nothing, NOT an error. This is
        // the regression guard: the edge-triggered Freeze effect is consumed once
        // at the lock transition; reconciling on `locked` re-freezes the child the
        // moment their session (slice) appears, instead of losing the freeze.
        assert_eq!(reconcile_freeze(true, false), None);
        assert_eq!(reconcile_freeze(false, false), None);
    }

    #[test]
    fn open_window_is_not_locked() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let rem = compute_remaining(&inp);
        assert!(!rem.locked);
        assert_eq!(rem.effective_secs, 2 * 3600); // 18:00 -> 20:00 close
    }

    #[test]
    fn lock_at_schedule_boundary() {
        let (usage, ext) = ledgers();
        let s = after_school();
        let mut core = EnforcerCore::new();
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: AFTER_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let effs = core.tick(&inp);
        assert!(core.is_locked());
        assert!(effs
            .iter()
            .any(|e| matches!(e, EnforcerEffect::Freeze(FreezeTarget::ManagedAppSlice))));
        assert!(effs
            .iter()
            .any(|e| matches!(e, EnforcerEffect::ShowLock(LockReason::Schedule))));
    }

    #[test]
    fn effective_is_min_of_window_and_quota() {
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 110 * 60); // used 110 of 120 daily
        let ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let rem = compute_remaining(&inp);
        // window-left = 3h; quota-left = 10 min -> min = 10 min.
        assert_eq!(rem.effective_secs, 10 * 60);
        assert_eq!(rem.reason, None);
    }

    #[test]
    fn budget_extension_thaws_on_reeval() {
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 120 * 60); // quota spent
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        let mut core = EnforcerCore::new();

        // Locks on budget.
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        core.tick(&inp);
        assert!(core.is_locked());

        // Guardian grants +15 min on the budget dimension.
        ext.apply(IN_WINDOW, "req-x", 15, crate::extension::Dimension::Budget);
        let inp2 = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let effs = core.tick(&inp2);
        assert!(!core.is_locked());
        assert!(effs.iter().any(|e| matches!(e, EnforcerEffect::Thaw(_))));
    }

    #[test]
    fn a_granted_extension_is_spent_and_relocks() {
        // The headline interaction: a ward runs out, the guardian gives 15
        // minutes, the ward uses them, and the lock comes BACK.
        //
        // Regression (found on Rob's laptop 2026-07-31): the extension was
        // added AFTER the budget floored at zero, so no amount of further use
        // could eat into it. `budget` sat at exactly the granted figure until
        // midnight and the ward was never re-locked — every "more time" grant
        // was really "free until tomorrow".
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 120 * 60); // quota exactly spent
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        macro_rules! remaining {
            () => {
                compute_remaining(&EnforcerInputs {
                    stand_down: None,
                    now_unix: IN_WINDOW,
                    schedule: Some(&s),
                    budget: Some(&b),
                    usage: &usage,
                    extension: &ext,
                    consolidated: None,
                })
            };
        }

        // Out of budget -> locked.
        assert!(remaining!().locked);

        // +15 minutes -> exactly 15 minutes back.
        ext.apply(IN_WINDOW, "req-x", 15, crate::extension::Dimension::Budget);
        let rem = remaining!();
        assert!(!rem.locked);
        assert_eq!(rem.budget_secs, 15 * 60, "the whole grant is available");

        // Spend ten of them -> five left, still unlocked.
        usage.credit(IN_WINDOW, Activity::Active, 10 * 60);
        let rem = remaining!();
        assert!(!rem.locked);
        assert_eq!(rem.budget_secs, 5 * 60, "the grant is being SPENT");

        // Spend the rest -> locked again, on budget.
        usage.credit(IN_WINDOW, Activity::Active, 5 * 60);
        let rem = remaining!();
        assert!(rem.locked, "granted time must run out");
        assert_eq!(rem.reason, Some(LockReason::Budget));
    }

    #[test]
    fn a_grant_to_an_overdrawn_ward_buys_the_minutes_it_says() {
        // decented's case, 2026-07-31: Rob 2h43m past a 2h cap, guardian taps
        // "approve 30 minutes". Before the baseline the grant was swallowed
        // whole by the existing deficit — the clause was signed and delivered,
        // the app said it worked, and the laptop stayed locked. "Give 30
        // minutes" must mean 30 minutes of real screen time.
        let over = (120 + 163) * 60;
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, over);
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        macro_rules! remaining {
            () => {
                compute_remaining(&EnforcerInputs {
                    stand_down: None,
                    now_unix: IN_WINDOW,
                    schedule: Some(&s),
                    budget: Some(&b),
                    usage: &usage,
                    extension: &ext,
                    consolidated: None,
                })
            };
        }

        assert!(remaining!().locked, "overdrawn to start with");

        ext.apply(IN_WINDOW, "req-a", 30, crate::extension::Dimension::Budget);
        let (t, w) = (usage.used_today(IN_WINDOW), usage.used_week(IN_WINDOW));
        ext.note_budget_baseline(IN_WINDOW, t, w);
        let rem = remaining!();
        assert!(!rem.locked, "the grant must actually reach him");
        assert_eq!(rem.budget_secs, 30 * 60, "30 minutes means 30 minutes");

        usage.credit(IN_WINDOW, Activity::Active, 30 * 60);
        assert!(remaining!().locked, "and then they run out");
    }

    #[test]
    fn a_second_grant_stacks_on_the_same_floor() {
        // Two grants of 15 give 30 usable minutes, not a moving goalpost that
        // re-forgives the overdraft each time.
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 200 * 60);
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };

        ext.apply(IN_WINDOW, "g1", 15, crate::extension::Dimension::Budget);
        let (t1, w1) = (usage.used_today(IN_WINDOW), usage.used_week(IN_WINDOW));
        ext.note_budget_baseline(IN_WINDOW, t1, w1);
        usage.credit(IN_WINDOW, Activity::Active, 15 * 60);

        ext.apply(IN_WINDOW, "g2", 15, crate::extension::Dimension::Budget);
        let (t2, w2) = (usage.used_today(IN_WINDOW), usage.used_week(IN_WINDOW));
        ext.note_budget_baseline(IN_WINDOW, t2, w2);

        let rem = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert_eq!(
            rem.budget_secs,
            15 * 60,
            "the second grant, not a fresh slate"
        );
    }

    #[test]
    fn an_extension_cannot_be_outrun_by_prior_overdraft() {
        // A ward already far past the cap (only reachable via a mid-day limit
        // cut or pooled usage) still gets nothing for free, but the grant is
        // not silently swallowed either: it lifts them by exactly its size.
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 185 * 60); // 65 min OVER 120
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(120),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        ext.apply(IN_WINDOW, "req-y", 5, crate::extension::Dimension::Budget);
        let rem = compute_remaining(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        // 120 + 5 - 185 is negative -> still locked. The guardian sees the
        // overdraft and can give enough to clear it if they mean to.
        assert!(
            rem.locked,
            "a grant smaller than the overdraft must not unlock"
        );
    }

    #[test]
    fn malformed_clause_at_bedtime_stays_locked() {
        // Fail-SAFE: a clause with a bad tz must lock, NOT fail-open.
        let (usage, ext) = ledgers();
        let mut s = after_school();
        s.tz = "Not/AZone".into();
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let rem = compute_remaining(&inp);
        assert!(rem.locked);
        assert_eq!(rem.reason, Some(LockReason::Malformed));
    }

    #[test]
    fn malformed_clause_with_schedule_extension_stays_locked() {
        // M1 fail-SAFE regression: a same-day schedule extension must NOT mask a
        // malformed clause. Before the fix, schedule_secs = (0 + ext).min(eod) > 0
        // unlocked the session on an unparseable bedtime clause.
        let (usage, mut ext) = ledgers();
        ext.apply(
            IN_WINDOW,
            "req-ext",
            30,
            crate::extension::Dimension::Schedule,
        );
        let mut s = after_school();
        s.tz = "Not/AZone".into();
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let rem = compute_remaining(&inp);
        assert!(
            rem.locked,
            "malformed clause must lock despite an extension"
        );
        assert_eq!(rem.reason, Some(LockReason::Malformed));
        assert_eq!(rem.schedule_secs, 0);
    }

    #[test]
    fn eod_robust_across_dst_spring_and_fall() {
        // M10: end-of-day must follow the calendar date, not now+24h.
        use chrono::TimeZone;
        let tz: Tz = "Europe/London".parse().unwrap();
        // Eve of spring-forward (29 Mar 2026): 23:30 -> 30 min to midnight.
        let spring = tz
            .with_ymd_and_hms(2026, 3, 28, 23, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(end_of_day_unix("Europe/London", spring) - spring, 1800);
        // Fall-back day (25 Oct 2026 is 25h long): 00:30 -> 24h30m to next midnight.
        let fall = tz
            .with_ymd_and_hms(2026, 10, 25, 0, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(end_of_day_unix("Europe/London", fall) - fall, 88_200);
    }

    #[test]
    fn next_open_populated_when_schedule_locked() {
        // F8: a schedule lock surfaces seconds-to-next-window for the lock screen.
        let (usage, ext) = ledgers();
        let s = after_school();
        let inp = EnforcerInputs {
            stand_down: None,
            now_unix: AFTER_WINDOW,
            schedule: Some(&s),
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        };
        let rem = compute_remaining(&inp);
        assert!(rem.locked);
        assert_eq!(rem.reason, Some(LockReason::Schedule));
        assert!(
            rem.next_open_secs.is_some_and(|s| s > 0),
            "schedule lock must report when the next window opens"
        );
    }

    #[test]
    fn warn_ten_rearms_after_extension_without_lock() {
        // M17: an extension lifts effective back above 10 min, then it drops
        // again — the warning must re-fire even without an intervening lock.
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 11 * 60); // 9 of 20 min left
        let mut ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(20),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        let mut core = EnforcerCore::new();
        let fired_ten = |effs: &[EnforcerEffect]| {
            effs.iter()
                .any(|e| matches!(e, EnforcerEffect::Warn(WarnLevel::Ten)))
        };

        let e1 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(fired_ten(&e1), "540s left should warn");

        ext.apply(IN_WINDOW, "req-ext", 2, crate::extension::Dimension::Budget); // -> 660s
        let e2 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(
            !fired_ten(&e2),
            "above 10 min should not warn (and re-arms)"
        );

        usage.credit(IN_WINDOW, Activity::Active, 3 * 60); // -> 480s effective
        let e3 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(fired_ten(&e3), "dropping back below 10 min must warn again");
    }

    #[test]
    fn warn_ten_rearms_after_unbounded_spell() {
        // Re-arm must also clear across an UNBOUNDED spell (effective == -1, e.g.
        // both clauses removed), not just a bounded rising edge.
        let mut usage = UsageLedger::new(TZ, WeekStart::Mon, IN_WINDOW);
        usage.credit(IN_WINDOW, Activity::Active, 11 * 60); // 9 of 20 min left
        let ext = ExtensionLedger::new(TZ, IN_WINDOW);
        let s = after_school();
        let b = GrantBudget {
            model: None,
            v: 1,
            tz: TZ.into(),
            daily_minutes: Some(20),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at: 1,
        };
        let mut core = EnforcerCore::new();
        let fired_ten = |effs: &[EnforcerEffect]| {
            effs.iter()
                .any(|e| matches!(e, EnforcerEffect::Warn(WarnLevel::Ten)))
        };

        let e1 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(fired_ten(&e1), "540s left should warn");

        // Both clauses removed -> unbounded (effective == -1) -> must re-arm.
        let e2 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: None,
            budget: None,
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(!fired_ten(&e2));

        let e3 = core.tick(&EnforcerInputs {
            stand_down: None,
            now_unix: IN_WINDOW,
            schedule: Some(&s),
            budget: Some(&b),
            usage: &usage,
            extension: &ext,
            consolidated: None,
        });
        assert!(
            fired_ten(&e3),
            "warning must re-fire after an unbounded spell"
        );
    }

    #[test]
    fn freeze_target_guard_accepts_user_slice_rejects_root_system_and_lock() {
        assert!(is_valid_freeze_target("user.slice/user-1000.slice"));
        assert!(!is_valid_freeze_target("user.slice/user-0.slice")); // never root
        assert!(!is_valid_freeze_target("charterd.service"));
        assert!(!is_valid_freeze_target("system.slice"));
        assert!(!is_valid_freeze_target(
            "user.slice/user-1000.slice/charter-lock.scope"
        )); // ends .scope + contains lock
        assert!(!is_valid_freeze_target("init.scope"));
    }
}
