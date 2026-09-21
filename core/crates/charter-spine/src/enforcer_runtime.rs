//! Wires the pure `EnforcerCore` over the system ports: it credits active
//! usage, reads the authenticated cached clauses, computes the decision, and
//! returns the effects for the platform's enforce half to apply (cgroup
//! freeze and VT lock on Linux; `setPackagesSuspended` and LockTask on
//! Android), then persists the usage + extension snapshots. Enforcement is
//! fail-SAFE: a present-but-malformed schedule OR budget clause locks (see
//! `child_policy`'s module doc for why the budget cannot be the exception).
//! The Linux-only halves — `managed_freeze_target` (uid → cgroup slice) and
//! `apply_effects` (the freeze/VT port applier) — stay in
//! `charterd::enforcer_runtime`.

use charter_proto::ClauseKind;
use charter_schedule::{
    Activity, Dimension, EnforcerCore, EnforcerEffect, EnforcerInputs, ExtensionLedger,
    GrantBudget, GrantSchedule, LockReason, Remaining, UsageLedger, WeekStart,
};
use charter_sys::persistence::{ClauseStore, ExtensionStore, UsageStore};
use charter_sys::{Clock, SystemLayer};

use crate::child_policy::{fail_safe_budget, fail_safe_schedule, FAIL_SAFE_TZ};

/// The runtime enforcer: owns the durable ledgers + the decision state machine.
pub struct EnforcerRuntime {
    core: EnforcerCore,
    usage: UsageLedger,
    extension: ExtensionLedger,
    tz: String,
}

impl EnforcerRuntime {
    /// Construct, reloading the usage + extension snapshots (restart-durable).
    pub fn new<S: SystemLayer>(sys: &S, tz: &str) -> Self {
        let now = sys.clock().now_utc() as i64;
        let usage = sys
            .usage()
            .load_snapshot()
            .ok()
            .flatten()
            .and_then(|s| UsageLedger::from_snapshot(&s))
            .unwrap_or_else(|| UsageLedger::new(tz, WeekStart::Mon, now));
        let extension = sys
            .extension()
            .load_snapshot()
            .ok()
            .flatten()
            .and_then(|s| ExtensionLedger::from_snapshot(&s))
            .unwrap_or_else(|| ExtensionLedger::new(tz, now));
        EnforcerRuntime {
            core: EnforcerCore::new(),
            usage,
            extension,
            tz: tz.to_string(),
        }
    }

    /// Construct with fresh ledgers (no snapshot load) — used when a shared
    /// handle must exist before the system layer (e.g. wiring the time.extend
    /// enactor into the broker registry).
    pub fn fresh(tz: &str, now_unix: i64) -> Self {
        EnforcerRuntime {
            core: EnforcerCore::new(),
            usage: UsageLedger::new(tz, WeekStart::Mon, now_unix),
            extension: ExtensionLedger::new(tz, now_unix),
            tz: tz.to_string(),
        }
    }

    /// The enforcement timezone.
    pub fn tz(&self) -> &str {
        &self.tz
    }

    fn load_schedule<S: SystemLayer>(&self, sys: &S) -> Option<GrantSchedule> {
        let json = sys
            .clauses()
            .get_clause(ClauseKind::Schedule.store_key())
            .ok()
            .flatten()?;
        match serde_json::from_str::<GrantSchedule>(&json) {
            Ok(s) => Some(s),
            // Fail-SAFE: an authenticated-but-unparseable schedule locks.
            Err(_) => Some(fail_safe_schedule(&self.tz)),
        }
    }

    /// The cached budget clause, or the fail-SAFE stand-in when one is there
    /// and will not parse.
    ///
    /// This used to be `.ok()`, i.e. an unparseable budget became `None`,
    /// which `compute_remaining` reads as NO CAP AT ALL. The justification was
    /// that the schedule remains the fail-safe gate — true only for a child
    /// who has a schedule. A budget-only charter ("2 hours a day, any time")
    /// therefore enforced nothing at all the moment the blob stopped parsing,
    /// and a value as ordinary as `"dailyMinutes": -5` against `Option<u32>`
    /// is enough to get there. Absent still means `None`: a guardian who set
    /// no budget has not lost one.
    fn load_budget<S: SystemLayer>(&self, sys: &S) -> Option<GrantBudget> {
        let json = sys
            .clauses()
            .get_clause(ClauseKind::Budget.store_key())
            .ok()
            .flatten()?;
        match serde_json::from_str::<GrantBudget>(&json) {
            Ok(b) => Some(b),
            Err(_) => Some(fail_safe_budget(&self.tz)),
        }
    }

    /// Decide one tick: credit usage, evaluate, persist the snapshots, and
    /// return the effects. **Synchronous** — so a shared `Mutex<EnforcerRuntime>`
    /// is never held across an `.await`. The async port effects (freeze/thaw/
    /// lock) are applied separately via [`apply_effects`].
    pub fn tick<S: SystemLayer>(
        &mut self,
        sys: &S,
        activity: Activity,
        elapsed_secs: u64,
    ) -> Vec<EnforcerEffect> {
        let now = sys.clock().now_utc() as i64;
        let schedule = self.load_schedule(sys);
        let budget = self.load_budget(sys);
        // M12/M13: the usage ledger resets on the BUDGET clause's tz + weekStart
        // (the guardian's authority), not the daemon's ambient runtime tz. A
        // changed tz/weekStart re-keys WITHOUT refilling spent quota.
        if let Some(b) = budget.as_ref() {
            self.usage
                .reconcile(&b.tz, b.week_start.unwrap_or(WeekStart::Mon), now);
        }
        // The today-only extension pool must roll on the SAME tz the enforcer
        // caps it against (schedule-first enforcement tz), not the stale runtime
        // tz — else a granted extension expires early or lingers past midnight.
        let enforcement_tz = schedule
            .as_ref()
            .map(|s| s.tz.clone())
            .or_else(|| budget.as_ref().map(|b| b.tz.clone()))
            .unwrap_or_else(|| self.tz.clone());
        self.extension.reconcile(&enforcement_tz, now);
        self.usage.credit(now, activity, elapsed_secs);
        // Out-of-window, a granted schedule extension burns wall-clock (the
        // fail-open fix: it must count down and re-lock, not freeze).
        charter_schedule::burn_schedule_extension(
            &mut self.extension,
            schedule.as_ref(),
            now,
            elapsed_secs,
        );
        let inp = EnforcerInputs {
            // Device-only limits have no stand-down: this store holds the
            // LOCAL fallback charter, and a stand-down is by definition a
            // guardian act. The broker routes clause 13 per-child, so a real
            // one arrives in the child-clause store and is enforced on the
            // multi-child path (see multi_child::set_stand_down). Not a stub.
            stand_down: None,
            now_unix: now,
            schedule: schedule.as_ref(),
            budget: budget.as_ref(),
            usage: &self.usage,
            extension: &self.extension,
            consolidated: None,
        };
        let effects = self.core.tick(&inp);
        let _ = sys.usage().save_snapshot(&self.usage.snapshot());
        let _ = sys.extension().save_snapshot(&self.extension.snapshot());
        effects
    }

    /// Phase-7 seam: push a today-only additive extension entry. Returns true if
    /// newly applied. It does NOT redefine the enforcer math.
    pub fn apply_extension(
        &mut self,
        now_unix: i64,
        req_id: &str,
        minutes: u16,
        dim: Dimension,
    ) -> bool {
        // Persisted on the immediately-following re-eval tick (the extension
        // flow triggers one). Fully durable-before-ack would require threading a
        // persistence port into the enactor — deferred (one-tick window).
        let newly = self.extension.apply(now_unix, req_id, minutes, dim);
        if newly && dim == Dimension::Budget {
            let (t, w) = (
                self.usage.used_today(now_unix),
                self.usage.used_week(now_unix),
            );
            self.extension.note_budget_baseline(now_unix, t, w);
        }
        newly
    }

    /// Phase-7 seam mirror for a per-group (`LimitHit::Bucket`) `time.extend`:
    /// push a today-only additive extension entry into the named bucket's own
    /// pool via `ExtensionLedger::apply_bucket`. Returns true if newly
    /// applied. Deliberately does NOT call `note_budget_baseline` — that
    /// floor exists only for the whole-device budget dimension.
    pub fn apply_bucket(
        &mut self,
        now_unix: i64,
        req_id: &str,
        minutes: u16,
        bucket_id: &str,
    ) -> bool {
        self.extension
            .apply_bucket(now_unix, req_id, minutes, bucket_id)
    }

    /// The current time-left breakdown (for the D-Bus `TimeLeft`).
    pub fn time_left<S: SystemLayer>(&self, sys: &S) -> Remaining {
        let now = sys.clock().now_utc() as i64;
        let schedule = self.load_schedule(sys);
        let budget = self.load_budget(sys);
        charter_schedule::compute_remaining(&EnforcerInputs {
            // Device-only limits have no stand-down: this store holds the
            // LOCAL fallback charter, and a stand-down is by definition a
            // guardian act. The broker routes clause 13 per-child, so a real
            // one arrives in the child-clause store and is enforced on the
            // multi-child path (see multi_child::set_stand_down). Not a stub.
            stand_down: None,
            now_unix: now,
            schedule: schedule.as_ref(),
            budget: budget.as_ref(),
            usage: &self.usage,
            extension: &self.extension,
            consolidated: None,
        })
    }

    pub fn is_locked(&self) -> bool {
        self.core.is_locked()
    }
}

/// End-of-day unix in the *clause* enforcement tz (loads the cached schedule +
/// budget clauses). The time.extend enactor's today-only cap must use THIS so it
/// agrees with `compute_remaining`, never the daemon's ambient runtime tz.
pub fn time_extend_eod<S: SystemLayer>(sys: &S, now_unix: i64) -> i64 {
    // A clause that is PRESENT and will not parse is the same event here as
    // it is in `load_schedule`/`load_budget`: it stands in as its fail-safe,
    // so the enactor's end-of-day agrees with what the enforcer is about to
    // do rather than silently pretending the clause is absent.
    let schedule: Option<GrantSchedule> = sys
        .clauses()
        .get_clause(ClauseKind::Schedule.store_key())
        .ok()
        .flatten()
        .map(|j| serde_json::from_str(&j).unwrap_or_else(|_| fail_safe_schedule(FAIL_SAFE_TZ)));
    let budget: Option<GrantBudget> = sys
        .clauses()
        .get_clause(ClauseKind::Budget.store_key())
        .ok()
        .flatten()
        .map(|j| serde_json::from_str(&j).unwrap_or_else(|_| fail_safe_budget(FAIL_SAFE_TZ)));
    // The extension is dimension-isolated, but the grant `exp` may legitimately
    // be the end-of-day in EITHER clause's tz (a schedule extension uses the
    // schedule tz, a budget extension the budget tz). Cap at the LATER of the two
    // local midnights so a valid same-day grant is never wrongly rejected — the
    // enforcer re-keys the pools per tz, so this only bounds the today-only check.
    let sched_eod = schedule
        .as_ref()
        .map(|s| charter_schedule::enforcement_eod_unix(Some(s), None, now_unix));
    let budget_eod = budget
        .as_ref()
        .map(|b| charter_schedule::enforcement_eod_unix(None, Some(b), now_unix));
    match (sched_eod, budget_eod) {
        (Some(a), Some(b)) => a.max(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        // No clauses cached: fall back to UTC end-of-day.
        (None, None) => charter_schedule::enforcement_eod_unix(None, None, now_unix),
    }
}

/// The child-facing `(title, detail)` shown on the lock for each reason —
/// rendered verbatim by every warden's lock surface (Linux charter-lock,
/// Android LockActivity); never re-derived platform-side.
pub fn lock_message(reason: LockReason) -> (&'static str, &'static str) {
    match reason {
        LockReason::Schedule => (
            "Outside allowed hours",
            "Access resumes during your scheduled time.",
        ),
        LockReason::Budget => (
            "Time's up for today",
            "Your daily time limit has been reached.",
        ),
        // Fail-safe lock from a bad clause — say so without alarming the child.
        LockReason::Malformed => ("Locked", "Setup needs attention — ask your guardian."),
        // Names the person, because a person is the way out. "Time's up" would
        // send her to wait for a tomorrow that isn't coming, and an unexplained
        // lock in the middle of her own evening reads as the phone breaking.
        LockReason::StandDown => (
            "That's it for today",
            "Your guardian asked you to finish up. Talk to them to come back on.",
        ),
    }
}

/// True when this schedule blocks all time with no window ever opening — the
/// "off unless a guardian opens it" posture MyCharter writes for a device that
/// is nobody's daily driver (spec 2026-07-28). Wire-`paused` is the ONLY way to
/// say it: an all-empty `weekly` alone would read as "no schedule, always
/// allowed", which is why `scheduleToGrant` encodes the empty week explicitly.
pub fn schedule_is_dormant(schedule: Option<&charter_schedule::GrantSchedule>) -> bool {
    schedule.is_some_and(|s| s.paused == Some(true))
}

/// [`lock_message`], but able to tell a dormant device from a closed window.
///
/// Both lock as `LockReason::Schedule`, and they MUST keep doing so: the gift
/// and ask routing switch on that reason in three places (charterd's runtime,
/// the JNI warden, and a string compare in LockActivity), and a fourth reason
/// any of them didn't know about would charge the grant to the BUDGET pool —
/// which cannot open a schedule lock. The guardian would tap "Give time", be
/// told it was sent, and watch the device stay shut. So the reason is shared
/// and only the words differ.
pub fn lock_message_for(reason: LockReason, dormant: bool) -> (&'static str, &'static str) {
    if dormant && matches!(reason, LockReason::Schedule) {
        // Not "outside allowed hours": there are no allowed hours, and telling a
        // child to wait for a window that will never open is a promise the
        // device cannot keep. Name the one way in — a person.
        return (
            "This device is off",
            "Ask your guardian to open it for a while.",
        );
    }
    lock_message(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dormant_schedule() -> charter_schedule::GrantSchedule {
        let json = r#"{"v":1,"tz":"Europe/London","weekly":{},"paused":true,"issuedAt":1}"#;
        serde_json::from_str(json).expect("valid dormant schedule")
    }

    fn open_schedule() -> charter_schedule::GrantSchedule {
        let json = r#"{"v":1,"tz":"Europe/London","weekly":{"mon":[{"start":"09:00","end":"17:00"}]},"issuedAt":1}"#;
        serde_json::from_str(json).expect("valid schedule")
    }

    #[test]
    fn only_a_wire_paused_schedule_is_dormant() {
        assert!(schedule_is_dormant(Some(&dormant_schedule())));
        assert!(!schedule_is_dormant(Some(&open_schedule())));
        // No schedule at all is "always allowed", the exact opposite of dormant.
        assert!(!schedule_is_dormant(None));
    }

    #[test]
    fn a_dormant_device_is_not_told_to_wait_for_a_window() {
        let (title, detail) = lock_message_for(LockReason::Schedule, true);
        assert_eq!(title, "This device is off");
        assert!(detail.contains("guardian"));
        // The false promise must be gone, not merely reworded.
        assert!(!detail.contains("scheduled time"));
    }

    #[test]
    fn dormancy_changes_nothing_for_any_other_lock() {
        // A closed window on a device that DOES have allowed hours is unchanged.
        assert_eq!(
            lock_message_for(LockReason::Schedule, false),
            lock_message(LockReason::Schedule)
        );
        // And a dormant flag must never leak into another reason's copy: a spent
        // budget on a dormant device is still "time's up", not "device is off".
        for reason in [
            LockReason::Budget,
            LockReason::Malformed,
            LockReason::StandDown,
        ] {
            assert_eq!(lock_message_for(reason, true), lock_message(reason));
        }
    }

    /// A device-only charter with a budget and no schedule — "2 hours a day,
    /// any time", which is how a great many families set a single laptop up.
    #[cfg(feature = "mock")]
    fn budget_only_runtime(
        budget_json: Option<&str>,
    ) -> (charter_sys::MockSystem, EnforcerRuntime) {
        use charter_sys::persistence::ClauseStore;
        use charter_sys::SystemLayer;
        let sys = charter_sys::MockSystem::new(1_700_000_000);
        if let Some(j) = budget_json {
            sys.clauses()
                .put_clause(ClauseKind::Budget.store_key(), 100, j)
                .unwrap();
        }
        let rt = EnforcerRuntime::new(&sys, "Europe/London");
        (sys, rt)
    }

    #[test]
    #[cfg(feature = "mock")]
    fn a_budget_that_will_not_parse_locks_instead_of_lifting_the_cap() {
        // `.ok()` here used to turn an unreadable budget into `None`, which
        // `compute_remaining` reads as no cap at all. With no schedule to fall
        // back on there was then nothing left enforcing anything: no lock, no
        // reason, no audit, until someone noticed by hand.
        let (sys, rt) = budget_only_runtime(Some(
            r#"{"v":1,"tz":"Europe/London","dailyMinutes":1.5,"issuedAt":100}"#,
        ));
        let rem = rt.time_left(&sys);
        assert_eq!(rem.budget_secs, 0);
        assert!(rem.locked, "a present-but-unusable budget must lock");
    }

    #[test]
    #[cfg(feature = "mock")]
    fn no_budget_clause_at_all_is_still_unconstrained() {
        let (sys, rt) = budget_only_runtime(None);
        let rem = rt.time_left(&sys);
        assert_eq!(rem.budget_secs, -1);
        assert!(
            !rem.locked,
            "absent is not malformed — nothing was ever set"
        );
    }

    #[test]
    #[cfg(feature = "mock")]
    fn a_readable_budget_is_untouched() {
        let (sys, rt) = budget_only_runtime(Some(
            r#"{"v":1,"tz":"Europe/London","dailyMinutes":120,"issuedAt":100}"#,
        ));
        let rem = rt.time_left(&sys);
        assert_eq!(rem.budget_secs, 120 * 60);
        assert!(!rem.locked);
    }

    #[test]
    fn lock_message_is_child_facing_per_reason() {
        assert_eq!(
            lock_message(LockReason::Schedule).0,
            "Outside allowed hours"
        );
        assert_eq!(lock_message(LockReason::Budget).0, "Time's up for today");
        // The fail-safe lock is honest but not alarming.
        let (title, detail) = lock_message(LockReason::Malformed);
        assert_eq!(title, "Locked");
        assert!(detail.contains("guardian"));
    }
}
