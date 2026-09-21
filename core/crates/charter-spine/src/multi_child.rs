//! Multi-child device-only enforcement.
//!
//! A shared family computer with one login per child, each with **independent**
//! limits (their own allowed hours / curfew + daily cap). The rule is *whoever
//! is logged in counts*: the active session's child accrues time against **their**
//! budget, and each child is judged against **their own** schedule + cap. If two
//! kids use it together (a co-op game / watching), only one is logged in — they
//! pick whose account, and that child's time is what's charged.
//!
//! Each child gets an independent [`EnforcerCore`] + usage/extension ledgers, so
//! one being locked (past their curfew or over their cap) never affects another.
//! Pure — no I/O, no clock — so it is fully unit-tested under mocks; the daemon
//! loop ([`crate::runtime`]) supplies the active uid, the wall clock, and applies
//! the per-child freeze/lock.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use charter_proto::GrantLearning;
use charter_schedule::{
    compute_remaining, Activity, Bucket, ConsolidatedUsage, Dimension, EnforcerCore,
    EnforcerEffect, EnforcerInputs, ExtensionLedger, GrantBudget, GrantSchedule, LockReason,
    Remaining, StandDown, UsageLedger, WeekStart,
};

use crate::child_policy::{EffectivePolicy, PolicySource};

/// A guardian-approved `time.extend` grant waiting to be applied to the live
/// per-child enforcer. The broker's `time.extend` enactor deposits these; the
/// enforcement loop ([`crate::runtime`]) drains them each tick into the brokered
/// child's ledger — so a verified extension actually extends/unlocks the child,
/// not merely the D-Bus status readout.
#[derive(Debug, Clone)]
pub struct PendingExtension {
    pub req_id: String,
    pub minutes: u16,
    /// The whole-device dimension this credits. `None` exactly when
    /// `bucket_id` is `Some` — a per-group extend credits the named bucket's
    /// own pool instead of a whole-device dimension.
    pub dim: Option<Dimension>,
    /// The named bucket this credits, for a per-group (`LimitHit::Bucket`)
    /// `time.extend` grant. `None` for the ordinary whole-device routing.
    pub bucket_id: Option<String>,
    pub at: i64,
}

/// Thread-safe queue of pending extensions, shared from the broker's enactor to
/// the enforcement loop (which owns the [`MultiChildEnforcer`] and can't be held
/// across the broker's `.await`s).
pub type ExtensionInbox = Arc<Mutex<Vec<PendingExtension>>>;

/// The enforcement tz for a resolved policy: the schedule's tz (the enforcement-
/// gate authority), else the budget's, else UTC (an unconstrained child).
fn policy_tz(p: &EffectivePolicy) -> String {
    p.schedule
        .as_ref()
        .map(|s| s.tz.clone())
        .or_else(|| p.budget.as_ref().map(|b| b.tz.clone()))
        .unwrap_or_else(|| "UTC".into())
}

fn policy_week_start(p: &EffectivePolicy) -> WeekStart {
    p.budget
        .as_ref()
        .and_then(|b| b.week_start)
        .unwrap_or(WeekStart::Mon)
}

/// One child's independent enforcement state. The schedule/budget come from the
/// resolved [`EffectivePolicy`] (guardian-authoritative, else device-only, else
/// unconstrained) — either may be `None` (no constraint on that dimension).
struct ChildEnforcer {
    core: EnforcerCore,
    usage: UsageLedger,
    extension: ExtensionLedger,
    tz: String,
    schedule: Option<GrantSchedule>,
    budget: Option<GrantBudget>,
    learning: Option<GrantLearning>,
    source: PolicySource,
    /// The freshest verified cross-device usage view (USAGE_SYNC, B3) — the
    /// pooled-budget input the runtime refreshes from the store each tick.
    consolidated: Option<ConsolidatedUsage>,
    /// A guardian stand-down in force, refreshed from the clause store each
    /// tick exactly like `consolidated`. `None` = none standing.
    stand_down: Option<StandDown>,
    /// The `buckets` clause's own tz + weekStart, refreshed from the clause
    /// store each tick exactly like `consolidated`/`stand_down`. `None` = no
    /// buckets clause (or it failed to parse). Used ONLY as the week-roll
    /// authority when no budget clause governs this child — see the
    /// precedence comment in `tick_attributed`.
    buckets_tz: Option<String>,
    buckets_week_start: Option<WeekStart>,
}

impl ChildEnforcer {
    fn new(policy: &EffectivePolicy, now: i64) -> Self {
        let tz = policy_tz(policy);
        let week_start = policy_week_start(policy);
        ChildEnforcer {
            core: EnforcerCore::new(),
            usage: UsageLedger::new(&tz, week_start, now),
            extension: ExtensionLedger::new(&tz, now),
            tz,
            schedule: policy.schedule.clone(),
            budget: policy.budget.clone(),
            learning: policy.learning.clone(),
            source: policy.source,
            consolidated: None,
            stand_down: None,
            buckets_tz: None,
            buckets_week_start: None,
        }
    }

    /// Replace the effective policy (a parent edit, or a guardian clause now
    /// supersedes device-only) — keep the accrued ledgers across the flip.
    fn relimit(&mut self, policy: &EffectivePolicy) {
        self.tz = policy_tz(policy);
        self.schedule = policy.schedule.clone();
        self.budget = policy.budget.clone();
        self.learning = policy.learning.clone();
        self.source = policy.source;
    }

    /// The bucket this child's tick actually credits, given the ATTRIBUTED
    /// bucket. Fail-closed: Learning only sticks when a learning clause is in
    /// force and (if capped) the cap isn't spent — otherwise it charges Screen.
    fn effective_bucket(&self, attributed: Bucket, now: i64) -> Bucket {
        if attributed != Bucket::Learning {
            return Bucket::Screen;
        }
        let Some(l) = self.learning.as_ref().filter(|l| !l.is_paused()) else {
            return Bucket::Screen;
        };
        match l.cap_minutes {
            Some(cap) if self.usage.learning_today_secs(now) >= u64::from(cap) * 60 => {
                Bucket::Screen
            }
            _ => Bucket::Learning,
        }
    }

    /// Restore the usage ledger from a persisted snapshot (restart durability).
    fn restore_usage(&mut self, snapshot: &str) {
        if let Some(u) = UsageLedger::from_snapshot(snapshot) {
            self.usage = u;
        }
    }

    /// Restore the extension ledger from a persisted snapshot.
    ///
    /// Without this a restart rebuilt the ledger EMPTY, and the two halves of
    /// that both hurt. A guardian-approved `time.extend` was simply lost — the
    /// grant's id was already burned at verify, so it could not be
    /// re-delivered either, and the child stayed locked while the guardian's
    /// phone and the D-Bus readout both said the time had been given. And
    /// `schedule_consumed_secs`, the only counter that burns down an
    /// out-of-window schedule gift, went with it: a ward given "+30 minutes
    /// past bedtime" could reboot (which polkit deliberately allows them) and
    /// come back to a full, unspent 30 minutes, as often as they liked until
    /// the gift expired.
    ///
    /// A snapshot that will not parse starts empty and says so. That is the
    /// fail-closed direction here — less time for the ward, not more — and it
    /// is the same direction `restore_usage` takes for the same reason.
    fn restore_extension(&mut self, snapshot: &str) {
        match ExtensionLedger::from_snapshot(snapshot) {
            Some(e) => self.extension = e,
            None => eprintln!(
                "charter: an extension snapshot would not parse — starting the pools empty, \
                 so any time already given today must be given again"
            ),
        }
    }
}

/// What the loop should do for one child this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildDecision {
    pub uid: u32,
    /// True if this child is the active (foreground) session.
    pub active: bool,
    /// True if this child is currently locked (past curfew or over their cap).
    pub locked: bool,
    /// WHY they are locked, as level state. The `ShowLock(reason)` effect is
    /// edge-triggered (fires once when the child crosses into locked), but the
    /// panel spawns when the child becomes FOREGROUND — often ticks later — so
    /// the lock copy must not depend on catching the edge.
    pub reason: Option<LockReason>,
    /// The freeze/lock effects to apply to **this child's** slice/session.
    pub effects: Vec<EnforcerEffect>,
    /// Which policy source governs this child (guardian / device-only /
    /// unconstrained) — operational visibility + the future STATUS feed.
    pub source: PolicySource,
}

/// Independent enforcement across every managed child.
#[derive(Default)]
pub struct MultiChildEnforcer {
    children: BTreeMap<u32, ChildEnforcer>,
}

impl MultiChildEnforcer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The managed uids currently tracked.
    pub fn uids(&self) -> Vec<u32> {
        self.children.keys().copied().collect()
    }

    /// Reconcile the tracked children with the resolved per-child policies: add
    /// new children, re-limit existing ones (keeping their accrued time), drop
    /// any no longer present. Each policy is the [`EffectivePolicy`] from
    /// [`crate::child_policy::resolve_effective`] (guardian-authoritative, else
    /// device-only, else unconstrained). `restore` optionally seeds a new
    /// child's ledgers from persisted snapshots, as `(usage, extension)` — the
    /// same pair, in the same order, that [`snapshots`](Self::snapshots)
    /// hands the loop to write. Both halves matter: the usage ledger holds
    /// what the ward has spent, the extension ledger what the guardian has
    /// given and how much of it is gone.
    ///
    /// This runs on more than a daemon restart. `retain` below evicts a child
    /// who is missing from `configs` for even one tick, and they are rebuilt
    /// from scratch on the next — so a ledger that is not restored here is a
    /// ledger that can be lost without anything crashing.
    pub fn sync(
        &mut self,
        configs: &[(u32, EffectivePolicy)],
        now: i64,
        restore: impl Fn(u32) -> (Option<String>, Option<String>),
    ) {
        let keep: std::collections::BTreeSet<u32> = configs.iter().map(|(u, _)| *u).collect();
        self.children.retain(|u, _| keep.contains(u));
        for (uid, policy) in configs {
            match self.children.get_mut(uid) {
                Some(c) => c.relimit(policy),
                None => {
                    let mut child = ChildEnforcer::new(policy, now);
                    let (usage, extension) = restore(*uid);
                    if let Some(snap) = usage {
                        child.restore_usage(&snap);
                    }
                    if let Some(snap) = extension {
                        child.restore_extension(&snap);
                    }
                    self.children.insert(*uid, child);
                }
            }
        }
    }

    /// Advance one tick with the active child's time attributed to `bucket`
    /// (from foreground-window attribution). A Learning attribution only
    /// credits the learning meter when that child has a live learning clause
    /// with cap headroom — anything else charges their screen budget.
    /// `active_uid` is the foreground session's user (the only one whose time
    /// is charged); `elapsed` is the seconds since the last tick. Returns an
    /// independent decision per child.
    pub fn tick_bucket(
        &mut self,
        active_uid: Option<u32>,
        now: i64,
        elapsed: u64,
        bucket: Bucket,
    ) -> Vec<ChildDecision> {
        self.tick_attributed(active_uid, now, elapsed, bucket, None, false)
    }

    /// [`tick_bucket`](Self::tick_bucket) plus the named app bucket ("Play")
    /// the foreground app belongs to, if any. That meter is credited ALONGSIDE
    /// the ordinary screen credit — an hour of Minecraft is an hour of screen
    /// time AND an hour of Play — so a capped bucket never makes time free the
    /// way the learning bucket does.
    ///
    /// `unrecognised` (spec §2.3) is ALSO credited alongside the ordinary
    /// screen credit, never instead of it: `true` only when the foreground
    /// window's resolved identity was successfully read off `/proc` but
    /// matched nothing in the device's own inventory — software Charter does
    /// not know about at all, never merely an installed app in no group (the
    /// caller must pass `false` for that case).
    pub fn tick_attributed(
        &mut self,
        active_uid: Option<u32>,
        now: i64,
        elapsed: u64,
        bucket: Bucket,
        app_bucket: Option<&str>,
        unrecognised: bool,
    ) -> Vec<ChildDecision> {
        let one: Vec<String> = app_bucket.map(str::to_string).into_iter().collect();
        self.tick_open(active_uid, now, elapsed, bucket, Some(&one), unrecognised)
    }

    /// [`tick_attributed`](Self::tick_attributed) for a device that can have
    /// SEVERAL things open at once, and the entry point for
    /// [`TimeModel::Named`].
    ///
    /// `open_buckets` is every named allowance represented by what is open
    /// right now — on Linux, one entry per costing app with a window; on
    /// Android, the foreground package's, so at most one. It must already be
    /// **deduplicated**: two members of Play open together spend Play once,
    /// because wall-clock time is not duplicable and charging it twice is the
    /// single most indefensible thing a meter can do to a child.
    ///
    /// **`None` is not `Some(&[])`.** `Some(&[])` is "we looked, and nothing
    /// costing is open" — the free-by-absence case. `None` is "we could not
    /// look at all": no readable display. In `Named` that distinction is the
    /// difference between a free afternoon and an uncapped day, so it is
    /// carried in the type rather than left to a caller to remember (a caller
    /// that passed the empty slice for both is exactly how a Wayland seat
    /// became an unlimited device).
    ///
    /// # What the two models do differently here
    ///
    /// - [`TimeModel::Session`] — unchanged. The baseline charges for being at
    ///   the device, `effective_bucket` may waive it as learning, and every
    ///   open allowance is debited alongside. An unreadable display costs the
    ///   baseline here already, because the baseline does not depend on the
    ///   display.
    /// - [`TimeModel::Named`] — there is no baseline and no waiver. Seconds are
    ///   charged **only** when something costing is open, and then exactly
    ///   once no matter how many are. Nothing open costs nothing, so an idle
    ///   desktop — or one showing only apps the guardian never named — is free
    ///   by absence rather than by a grant that has to be defended. An
    ///   unreadable display is the one exception: it charges the `Session`
    ///   baseline (`Bucket::Screen`) and no app bucket, because a model whose
    ///   every cap is gated on a probe must not go inert when the probe does.
    ///
    /// # Why the baseline, and not the last known open set
    ///
    /// Charging what was open a tick ago would meter an app the ward may have
    /// closed, against a named allowance, on no evidence — a false accusation
    /// aimed at one specific app. The baseline claims less: it says only "the
    /// ward is at the device", which the active-session check already
    /// established without the display's help.
    ///
    /// # The minute journal follows the model
    ///
    /// `credit_bucket` marks the journal, and the journal is what the
    /// cross-device union pools. Marking a minute in which nothing cost would
    /// let free time on a laptop consume a phone's allowance — an over-charge
    /// arriving from a different device entirely. In `Named` the journal is
    /// therefore only touched on the costing path, which falls out of charging
    /// through the same call rather than needing its own rule to remember.
    pub fn tick_open(
        &mut self,
        active_uid: Option<u32>,
        now: i64,
        elapsed: u64,
        bucket: Bucket,
        open_buckets: Option<&[String]>,
        unrecognised: bool,
    ) -> Vec<ChildDecision> {
        let mut out = Vec::with_capacity(self.children.len());
        for (uid, c) in self.children.iter_mut() {
            // Roll the usage ledger's day/week keys on: the BUDGET clause's
            // tz + weekStart (the cap authority) when a budget clause is in
            // force; else the BUCKETS clause's own tz + weekStart, so a
            // buckets-only family's chosen weekStart is actually honoured
            // instead of silently defaulting to Monday; else the enforcement
            // tz + Monday (fully unconstrained). One shared week key cannot
            // honour two DIFFERENT weekStarts at once — when both a budget
            // and a buckets clause are in force, budget wins (MyCharter emits
            // one family-level weekStart into both clauses, so this is not
            // expected to actually diverge in practice). The extension pool
            // rolls on the enforcement (schedule-first) tz, unaffected.
            let (usage_tz, week_start) = match (c.budget.as_ref(), c.buckets_tz.as_deref()) {
                (Some(b), _) => (b.tz.clone(), b.week_start.unwrap_or(WeekStart::Mon)),
                (None, Some(tz)) => (
                    tz.to_string(),
                    c.buckets_week_start.unwrap_or(WeekStart::Mon),
                ),
                (None, None) => (c.tz.clone(), WeekStart::Mon),
            };
            c.usage.reconcile(&usage_tz, week_start, now);
            c.extension.reconcile(&c.tz, now);

            // Whoever is logged in counts: only the active child accrues time.
            let active = Some(*uid) == active_uid;
            if active {
                // WHAT the day's allowance is spent on. Absent from the clause
                // means Session — every charter signed before the field
                // existed keeps its exact meaning.
                let charge = match charter_schedule::TimeModel::of(c.budget.as_ref()) {
                    charter_schedule::TimeModel::Session => Some(c.effective_bucket(bucket, now)),
                    // No baseline, no waiver: something costing is open, or
                    // nothing is charged. Once, however many are open.
                    charter_schedule::TimeModel::Named => match open_buckets {
                        Some(open) => (!open.is_empty()).then_some(Bucket::Screen),
                        // The display could not be read. Fall back to the
                        // Session baseline for this tick — see the doc above:
                        // in this model every cap is gated on the probe, so a
                        // probe that can never succeed is otherwise a day with
                        // no limits at all.
                        None => Some(Bucket::Screen),
                    },
                };
                if let Some(eff) = charge {
                    c.usage.credit_bucket(now, Activity::Active, eff, elapsed);
                }
                // No display, no evidence about any PARTICULAR app: the
                // baseline above is charged, but no named allowance is spent
                // on a guess.
                for id in open_buckets.unwrap_or_default() {
                    c.usage.credit_app_bucket(now, id, elapsed);
                }
                // Credited in BOTH models, and in Named even when nothing was
                // charged. That is the point of it here: "something ran and
                // nothing in the charter says whether it costs" is exactly the
                // signal that the cost list has a hole, and Named makes an
                // out-of-date list the system's characteristic failure. A
                // counter that went quiet precisely when nothing was charged
                // would report a full cost list at the moment it was least
                // true.
                if unrecognised {
                    c.usage.credit_unrecognised(now, Activity::Active, elapsed);
                }
            }
            // Out-of-window, a granted schedule extension burns wall-clock —
            // for EVERY child, active or not: "+30 minutes past curfew" is a
            // wall-clock allowance, not a usage meter (the fail-open fix).
            charter_schedule::burn_schedule_extension(
                &mut c.extension,
                c.schedule.as_ref(),
                now,
                elapsed,
            );

            let inputs = EnforcerInputs {
                stand_down: c.stand_down,
                now_unix: now,
                schedule: c.schedule.as_ref(),
                budget: c.budget.as_ref(),
                usage: &c.usage,
                extension: &c.extension,
                consolidated: c.consolidated.as_ref(),
            };
            let effects = c.core.tick(&inputs);
            out.push(ChildDecision {
                uid: *uid,
                active,
                locked: c.core.is_locked(),
                reason: compute_remaining(&inputs).reason,
                effects,
                source: c.source,
            });
        }
        out
    }

    /// [`tick_bucket`](Self::tick_bucket) with Screen attribution — the exact
    /// pre-learning behaviour.
    pub fn tick(&mut self, active_uid: Option<u32>, now: i64, elapsed: u64) -> Vec<ChildDecision> {
        self.tick_bucket(active_uid, now, elapsed, Bucket::Screen)
    }

    /// Apply a today-only additive `time.extend` to a tracked child's ledger,
    /// idempotent by `req_id`. Returns true if newly applied; a no-op (false) if
    /// the uid isn't currently tracked. Called by the enforcement loop for
    /// guardian-approved extensions so an extension actually extends/unlocks the
    /// child on the **enforcing** ledger (the per-child `ExtensionLedger` that
    /// [`tick`](Self::tick) reads), not the standalone `EnforcerRuntime` the loop
    /// never consults.
    pub fn apply_extension(
        &mut self,
        uid: u32,
        now: i64,
        req_id: &str,
        minutes: u16,
        dim: Dimension,
    ) -> bool {
        match self.children.get_mut(&uid) {
            Some(c) => {
                let newly = c.extension.apply(now, req_id, minutes, dim);
                // Record where the debt stood, so a grant to an already
                // overdrawn ward buys the minutes it says instead of being
                // swallowed by the deficit. Only on a NEW budget grant: a
                // replayed reqId must not move the floor.
                if newly && dim == Dimension::Budget {
                    let (t, w) = (c.usage.used_today(now), c.usage.used_week(now));
                    c.extension.note_budget_baseline(now, t, w);
                }
                newly
            }
            None => false,
        }
    }

    /// Take a today-only `minutes` back off a tracked child's allowance —
    /// the exact mirror of [`apply_extension`](Self::apply_extension), sharing
    /// its idempotency by `req_id`. Returns true if newly applied; a no-op
    /// (false) if the uid isn't currently tracked.
    ///
    /// Deliberately does NOT touch the budget baseline: that floor exists so a
    /// grant to an overdrawn ward buys the minutes it says, and a take-back
    /// must not be able to establish one (which would quietly forgive an
    /// existing overdraft as a side effect of removing time).
    pub fn deduct_extension(
        &mut self,
        uid: u32,
        now: i64,
        req_id: &str,
        minutes: u16,
        dim: Dimension,
    ) -> bool {
        match self.children.get_mut(&uid) {
            Some(c) => c.extension.deduct(now, req_id, minutes, dim),
            None => false,
        }
    }

    /// The live time-left breakdown for one tracked child (the D-Bus status
    /// surface / `charter status`). `None` if the uid isn't tracked. Pure — reads
    /// the ledgers as of `now`, reconciling nothing; the tick keeps them fresh.
    pub fn remaining(&self, uid: u32, now: i64) -> Option<Remaining> {
        let c = self.children.get(&uid)?;
        Some(compute_remaining(&EnforcerInputs {
            stand_down: c.stand_down,
            now_unix: now,
            schedule: c.schedule.as_ref(),
            budget: c.budget.as_ref(),
            usage: &c.usage,
            extension: &c.extension,
            consolidated: c.consolidated.as_ref(),
        }))
    }

    /// Refresh a tracked child's cross-device usage view (USAGE_SYNC, B3).
    /// The runtime calls this each tick from the verified store; `None` clears
    /// (e.g. the stored view failed to parse). Untracked uids are a no-op.
    pub fn set_consolidated(&mut self, uid: u32, view: Option<ConsolidatedUsage>) {
        if let Some(c) = self.children.get_mut(&uid) {
            c.consolidated = view;
        }
    }

    /// Refresh a tracked child's guardian stand-down. The runtime calls this
    /// each tick from the clause store; `None` clears it (lifted, lapsed, or
    /// never set). Untracked uids are a no-op.
    pub fn set_stand_down(&mut self, uid: u32, sd: Option<StandDown>) {
        if let Some(c) = self.children.get_mut(&uid) {
            c.stand_down = sd;
        }
    }

    /// Refresh a tracked child's `buckets` clause tz + weekStart — the
    /// week-roll authority when no budget clause governs this child (see the
    /// precedence comment in `tick_attributed`). The runtime calls this each
    /// tick from the clause store, exactly like `set_consolidated`/
    /// `set_stand_down`. `(None, _)` clears it (no buckets clause this tick,
    /// or it failed to parse) so a removed clause falls back to the
    /// enforcement tz rather than sticking to a stale one. Untracked uids are
    /// a no-op.
    pub fn set_buckets_policy(
        &mut self,
        uid: u32,
        tz: Option<&str>,
        week_start: Option<WeekStart>,
    ) {
        if let Some(c) = self.children.get_mut(&uid) {
            c.buckets_tz = tz.map(String::from);
            c.buckets_week_start = week_start;
        }
    }

    /// The device's raw usage-today seconds for a child — a STATUS-feed
    /// aggregation input. `None` if the uid isn't tracked.
    pub fn used_today(&self, uid: u32, now: i64) -> Option<u64> {
        Some(self.children.get(&uid)?.usage.used_today(now))
    }

    /// Today's active-minutes journal for one tracked child as the STATUS
    /// wire string — `None` when untracked or the journal is empty (the field
    /// is omitted from STATUS rather than sent as an all-zero bitmap).
    pub fn minutes_today_b64(&self, uid: u32, now: i64) -> Option<String> {
        let m = self.children.get(&uid)?.usage.minutes_today(now);
        if m.is_empty() {
            None
        } else {
            Some(m.to_b64url())
        }
    }

    /// Learning-bucket seconds today for one tracked child (`None` untracked).
    pub fn learning_today(&self, uid: u32, now: i64) -> Option<u64> {
        Some(self.children.get(&uid)?.usage.learning_today_secs(now))
    }

    /// Unrecognised-time seconds today (spec §2.3) for one tracked child
    /// (`None` untracked) — the STATUS aggregate input.
    pub fn unrecognised_today(&self, uid: u32, now: i64) -> Option<u64> {
        Some(self.children.get(&uid)?.usage.unrecognised_today_secs(now))
    }

    /// Seconds spent in one named app bucket today, for a tracked child.
    pub fn app_bucket_today(&self, uid: u32, bucket_id: &str, now: i64) -> Option<u64> {
        Some(
            self.children
                .get(&uid)?
                .usage
                .app_bucket_today_secs(now, bucket_id),
        )
    }

    /// Seconds spent in one named app bucket THIS WEEK, for a tracked child —
    /// the week-keyed twin of [`app_bucket_today`](Self::app_bucket_today).
    pub fn app_bucket_week(&self, uid: u32, bucket_id: &str, now: i64) -> Option<u64> {
        Some(
            self.children
                .get(&uid)?
                .usage
                .app_bucket_week_secs(now, bucket_id),
        )
    }

    /// Today's pooled `time.extend`/gift extra for one named bucket, for a
    /// tracked child — lifts BOTH the day and week walls (the extra is a
    /// single today-only pool shared by both axes; see
    /// `ExtensionLedger::apply_bucket`). `None` if the uid isn't tracked.
    pub fn bucket_extra(&self, uid: u32, bucket_id: &str, now: i64) -> Option<u64> {
        Some(
            self.children
                .get(&uid)?
                .extension
                .bucket_extra_secs(now, bucket_id),
        )
    }

    /// Apply a today-only additive extension to a tracked child's NAMED
    /// BUCKET pool, idempotent by `req_id` — the per-group mirror of
    /// [`apply_extension`](Self::apply_extension). Returns true if newly
    /// applied; a no-op (false) if the uid isn't currently tracked. Deliberately
    /// does NOT call `note_budget_baseline` — that floor is whole-device
    /// budget machinery and has no bucket analogue.
    pub fn apply_bucket_extension(
        &mut self,
        uid: u32,
        now: i64,
        req_id: &str,
        minutes: u16,
        bucket_id: &str,
    ) -> bool {
        match self.children.get_mut(&uid) {
            Some(c) => c.extension.apply_bucket(now, req_id, minutes, bucket_id),
            None => false,
        }
    }

    /// The learning apps in force for one tracked child (empty when no clause /
    /// paused) — the loop feeds these to foreground attribution.
    pub fn learning_apps(&self, uid: u32) -> Vec<charter_proto::LearningApp> {
        self.children
            .get(&uid)
            .and_then(|c| c.learning.as_ref())
            // DEFINED, not active: this feeds launcher materialisation and the
            // sanctioned-launch predicate, neither of which is about free time.
            // The free grant's own pause gate lives in `effective_bucket`, so
            // a paused clause still yields Screen there — but a costing site's
            // window keeps existing, and keeps being spared by the lockdown.
            .map(charter_proto::GrantLearning::defined_apps)
            .unwrap_or_default()
    }

    /// Whether two instants fall in the same enforcement day for a tracked
    /// child, judged by that child's own extension ledger — the same tz and
    /// the same boundary its applied-id list is cleared on. `None` if the uid
    /// isn't tracked.
    ///
    /// For callers that replay a durable record of adjustments every tick and
    /// must not re-apply yesterday's. See `ExtensionLedger::same_day`.
    pub fn same_enforcement_day(&self, uid: u32, a: i64, b: i64) -> Option<bool> {
        Some(self.children.get(&uid)?.extension.same_day(a, b))
    }

    /// Per-child `(uid, usage_snapshot, extension_snapshot)` for the loop to
    /// persist (restart durability).
    pub fn snapshots(&self) -> Vec<(u32, String, String)> {
        self.children
            .iter()
            .map(|(uid, c)| (*uid, c.usage.snapshot(), c.extension.snapshot()))
            .collect()
    }

    /// Today's remaining budget seconds for a child (test helper).
    #[cfg(test)]
    fn used_secs(&self, uid: u32, now: i64) -> i64 {
        self.remaining(uid, now).unwrap().budget_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::child_policy::resolve_effective;
    use crate::local_limits::DeviceLimits;

    // 2024-01-03 is a Wednesday (a weekday). UTC so HH:MM == the clock.
    const WED_0000: i64 = 1_704_240_000;
    fn wed_at(hour: i64) -> i64 {
        WED_0000 + hour * 3600
    }

    fn limits(curfew_end: &str, daily_minutes: u32) -> DeviceLimits {
        DeviceLimits {
            tz: "UTC".into(),
            wake: "07:00".into(),
            bedtime: curfew_end.into(),
            daily_minutes,
            weekend: None,
        }
    }

    /// A device-only-sourced policy (the standalone path).
    fn child(curfew_end: &str, daily_minutes: u32) -> EffectivePolicy {
        resolve_effective(
            &charter_sys::persistence::ChildClauses::default(),
            Some(&limits(curfew_end, daily_minutes)),
            None,
        )
    }

    /// A guardian-sourced policy with the same numbers (Signet-first path).
    fn guardian_child(curfew_end: &str, daily_minutes: u32) -> EffectivePolicy {
        let l = limits(curfew_end, daily_minutes);
        EffectivePolicy {
            schedule: Some(l.to_schedule(1)),
            budget: Some(l.to_budget(1)),
            learning: None,
            source: PolicySource::Guardian,
        }
    }

    fn dec(decisions: &[ChildDecision], uid: u32) -> &ChildDecision {
        decisions
            .iter()
            .find(|d| d.uid == uid)
            .expect("child present")
    }

    const YOUNGER: u32 = 1001;
    const OLDER: u32 = 1002;

    /// A guardian policy whose schedule is the fail-SAFE paused one (what
    /// `child_policy::resolve_effective` produces from a malformed guardian
    /// schedule clause) — it must lock the child.
    fn fail_safe_guardian_policy() -> EffectivePolicy {
        EffectivePolicy {
            schedule: Some(GrantSchedule {
                v: 1,
                tz: "UTC".into(),
                paused: Some(true),
                weekly: charter_schedule::WeeklySchedule::default(),
                overrides: None,
                issued_at: 0,
            }),
            budget: None,
            learning: None,
            source: PolicySource::Guardian,
        }
    }

    /// A guardian policy that is out of window right now, with a budget —
    /// the shape a "+30 minutes past bedtime" gift lands on.
    fn after_bedtime_policy() -> EffectivePolicy {
        guardian_child("20:00", 120)
    }

    #[test]
    fn a_restart_keeps_the_time_the_guardian_gave_and_what_is_left_of_it() {
        // The extension snapshot was produced by `snapshots()` and thrown
        // away by the loop, and nothing ever read one back. So a reboot lost
        // an approved `time.extend` outright — unrecoverably, because the
        // grant's id is burned at verify — and refilled a schedule gift to
        // its FULL value, which a ward is allowed to do as often as they like
        // (polkit grants them reboot on purpose).
        let now = wed_at(21); // past the 20:00 bedtime: the window is shut
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, after_bedtime_policy())], now, |_| (None, None));
        assert!(
            e.remaining(YOUNGER, now).unwrap().locked,
            "out of window to begin with"
        );

        assert!(e.apply_extension(YOUNGER, now, "grant-a", 30, Dimension::Schedule));
        // Ten minutes of it are spent while the window is shut.
        e.tick(Some(YOUNGER), now + 600, 600);
        let before = e.remaining(YOUNGER, now + 600).unwrap();
        assert!(!before.locked, "the grant is holding the device open");

        // Persist, then rebuild from nothing — a reboot, a `.deb` upgrade, or
        // simply this child missing from `configs` for one tick.
        let snaps = e.snapshots();
        let (_, usage, ext) = snaps
            .iter()
            .find(|(u, _, _)| *u == YOUNGER)
            .unwrap()
            .clone();
        let mut fresh = MultiChildEnforcer::new();
        fresh.sync(&[(YOUNGER, after_bedtime_policy())], now + 600, |_| {
            (Some(usage.clone()), Some(ext.clone()))
        });

        let after = fresh.remaining(YOUNGER, now + 600).unwrap();
        assert_eq!(
            after.schedule_secs, before.schedule_secs,
            "the granted minutes survive the restart — and no more than that: \
             the ten already spent are still spent"
        );
        assert!(!after.locked);

        // And the grant cannot be applied a second time by a re-delivery,
        // which is the whole reason the applied-id list has to survive too.
        assert!(
            !fresh.apply_extension(YOUNGER, now + 600, "grant-a", 30, Dimension::Schedule),
            "a re-delivered grant id must be a no-op after the restore"
        );
        assert_eq!(
            fresh.remaining(YOUNGER, now + 600).unwrap().schedule_secs,
            after.schedule_secs
        );
    }

    #[test]
    fn a_restart_without_the_extension_snapshot_would_refill_the_gift() {
        // The control that names the old behaviour: restoring ONLY the usage
        // ledger (what the loop used to do) hands the ward the whole pool
        // back, unspent.
        let now = wed_at(21);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, after_bedtime_policy())], now, |_| (None, None));
        assert!(e.apply_extension(YOUNGER, now, "grant-a", 30, Dimension::Schedule));
        e.tick(Some(YOUNGER), now + 600, 600);
        let snaps = e.snapshots();
        let (_, usage, ext) = snaps
            .iter()
            .find(|(u, _, _)| *u == YOUNGER)
            .unwrap()
            .clone();

        let mut usage_only = MultiChildEnforcer::new();
        usage_only.sync(&[(YOUNGER, after_bedtime_policy())], now + 600, |_| {
            (Some(usage.clone()), None)
        });
        assert!(
            usage_only.remaining(YOUNGER, now + 600).unwrap().locked,
            "with no extension ledger the grant is simply gone"
        );
        assert!(
            usage_only.apply_extension(YOUNGER, now + 600, "grant-a", 30, Dimension::Schedule),
            "and the same id applies again — the applied list went with it"
        );

        // With the snapshot, neither happens.
        let mut both = MultiChildEnforcer::new();
        both.sync(&[(YOUNGER, after_bedtime_policy())], now + 600, |_| {
            (Some(usage.clone()), Some(ext.clone()))
        });
        assert!(!both.remaining(YOUNGER, now + 600).unwrap().locked);
        assert!(!both.apply_extension(YOUNGER, now + 600, "grant-a", 30, Dimension::Schedule));
    }

    #[test]
    fn an_extension_snapshot_that_will_not_parse_starts_empty() {
        // Fail-CLOSED for the ward: less time, not more. The alternative —
        // guessing at a pool we cannot read — would be inventing minutes.
        let now = wed_at(21);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, after_bedtime_policy())], now, |_| {
            (None, Some("{ not a ledger".to_string()))
        });
        assert!(e.remaining(YOUNGER, now).unwrap().locked);
    }

    #[test]
    fn the_budget_baseline_survives_a_restart_too() {
        // `note_budget_baseline` is the floor that lets a grant to an already
        // overdrawn ward buy the minutes it says. It lives in the extension
        // ledger, so it went with everything else.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, guardian_child("20:00", 60))], now, |_| {
            (None, None)
        });
        // Spend well past the 60-minute cap.
        e.tick(Some(YOUNGER), now + 2 * 3600, 2 * 3600);
        let t = now + 2 * 3600;
        assert!(e.remaining(YOUNGER, t).unwrap().locked, "overdrawn");
        assert!(e.apply_extension(YOUNGER, t, "grant-b", 20, Dimension::Budget));
        let before = e.remaining(YOUNGER, t).unwrap();
        assert_eq!(before.budget_secs, 20 * 60, "the grant means what it says");

        let snaps = e.snapshots();
        let (_, usage, ext) = snaps
            .iter()
            .find(|(u, _, _)| *u == YOUNGER)
            .unwrap()
            .clone();
        let mut fresh = MultiChildEnforcer::new();
        fresh.sync(&[(YOUNGER, guardian_child("20:00", 60))], t, |_| {
            (Some(usage.clone()), Some(ext.clone()))
        });
        assert_eq!(
            fresh.remaining(YOUNGER, t).unwrap().budget_secs,
            before.budget_secs,
            "and still means it after a restart, rather than being swallowed \
             again by the overdraft"
        );
    }

    #[test]
    fn consolidated_view_pools_the_budget_through_tick() {
        // B3: 30m local + a 35m spent-elsewhere view against a 60m budget must
        // lock; clearing the view (relay went quiet) unlocks — local-only again.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        // Budget-only policy: no curfew interference at 10:00 (07:00–20:00 open).
        e.sync(&[(YOUNGER, guardian_child("20:00", 60))], now, |_| {
            (None, None)
        });
        // 30 minutes of active use.
        let after = now + 30 * 60;
        e.tick(Some(YOUNGER), after, 30 * 60);
        assert!(!dec(&e.tick(Some(YOUNGER), after, 0), YOUNGER).locked);

        let mut view = ConsolidatedUsage {
            spent_elsewhere_today_secs: 35 * 60,
            ..ConsolidatedUsage::default()
        };
        view.day_key = {
            // The ledger's current local day (UTC in these tests).
            let dt = chrono::DateTime::from_timestamp(after, 0).unwrap();
            dt.format("%Y-%m-%d").to_string()
        };
        e.set_consolidated(YOUNGER, Some(view));
        let d = e.tick(Some(YOUNGER), after, 0);
        assert!(
            dec(&d, YOUNGER).locked,
            "30m here + 35m elsewhere must exhaust a 60m pooled budget"
        );
        e.set_consolidated(YOUNGER, None);
        assert!(!dec(&e.tick(Some(YOUNGER), after, 0), YOUNGER).locked);
    }

    /// The Linux half of "Finish now": a stand-down set on a tracked child must
    /// actually drive a lock through tick, and clearing it must bring them
    /// straight back. charterd shipped for hours with this input hardcoded to
    /// None, so a laptop took the clause and ignored it.
    #[test]
    fn a_stand_down_locks_a_child_through_tick_and_lifts_cleanly() {
        let now = wed_at(10); // mid-morning: allowed but for the stand-down
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, guardian_child("20:00", 60))], now, |_| {
            (None, None)
        });
        assert!(!dec(&e.tick(Some(YOUNGER), now, 0), YOUNGER).locked);

        // Still inside the grace: warned, not locked.
        e.set_stand_down(
            YOUNGER,
            Some(StandDown {
                secs_until_lock: 60,
            }),
        );
        assert!(!dec(&e.tick(Some(YOUNGER), now, 0), YOUNGER).locked);

        // Grace elapsed: locked, and attributed so the ward is told a PERSON is
        // the way out rather than being left to think the laptop broke.
        e.set_stand_down(YOUNGER, Some(StandDown { secs_until_lock: 0 }));
        let d = e.tick(Some(YOUNGER), now, 0);
        assert!(dec(&d, YOUNGER).locked);
        assert_eq!(dec(&d, YOUNGER).reason, Some(LockReason::StandDown));

        // Lifted (or lapsed): straight back, no residue.
        e.set_stand_down(YOUNGER, None);
        assert!(!dec(&e.tick(Some(YOUNGER), now, 0), YOUNGER).locked);
    }

    /// Per-child isolation: standing one ward down must not touch a sibling.
    #[test]
    fn a_stand_down_does_not_reach_a_sibling() {
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(
            &[
                (YOUNGER, guardian_child("20:00", 60)),
                (OLDER, guardian_child("20:00", 60)),
            ],
            now,
            |_| (None, None),
        );
        e.set_stand_down(YOUNGER, Some(StandDown { secs_until_lock: 0 }));
        let d = e.tick(Some(YOUNGER), now, 0);
        assert!(dec(&d, YOUNGER).locked);
        assert!(!dec(&d, OLDER).locked, "a sibling must be untouched");
    }

    #[test]
    fn fail_safe_paused_schedule_locks_child_through_tick() {
        // The resolve -> enforce composition for the malformed/fail-safe path:
        // a paused schedule actually drives a lock in MultiChildEnforcer::tick.
        let now = wed_at(10); // mid-morning — would be allowed but for the pause
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, fail_safe_guardian_policy())], now, |_| {
            (None, None)
        });
        let d = e.tick(Some(YOUNGER), now, 0);
        assert!(
            dec(&d, YOUNGER).locked,
            "a fail-safe paused guardian schedule must lock the child"
        );
    }

    #[test]
    fn one_childs_fail_safe_lock_does_not_affect_a_sibling() {
        // Per-child isolation of the malformed/fail-safe path: YOUNGER's paused
        // (locking) schedule leaves OLDER — a normal device-only child — untouched.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(
            &[
                (YOUNGER, fail_safe_guardian_policy()),
                (OLDER, child("20:00", 120)),
            ],
            now,
            |_| (None, None),
        );
        let d = e.tick(Some(OLDER), now, 0);
        assert!(dec(&d, YOUNGER).locked, "younger is fail-safe locked");
        assert!(
            !dec(&d, OLDER).locked,
            "older (a sibling) is unaffected by younger's bad clause"
        );
        assert_eq!(dec(&d, OLDER).source, PolicySource::DeviceOnly);
    }

    fn two_kids(now: i64) -> MultiChildEnforcer {
        let mut e = MultiChildEnforcer::new();
        // Younger: curfew 18:00, 60 min/day. Older: curfew 20:00, 120 min/day.
        e.sync(
            &[(YOUNGER, child("18:00", 60)), (OLDER, child("20:00", 120))],
            now,
            |_| (None, None),
        );
        e
    }

    #[test]
    fn independent_curfews_lock_each_child_separately() {
        let now = wed_at(19); // 19:00 — past younger's 18:00, before older's 20:00
        let mut e = two_kids(now);
        let d = e.tick(Some(OLDER), now, 0);
        assert!(
            dec(&d, YOUNGER).locked,
            "younger is past their 18:00 curfew"
        );
        assert!(
            !dec(&d, OLDER).locked,
            "older is within their 20:00 curfew and under cap"
        );
    }

    /// A device-only child with a guardian learning clause layered on.
    fn learning_child(cap_minutes: Option<u32>, paused: Option<bool>) -> EffectivePolicy {
        let mut p = child("20:00", 120);
        p.learning = Some(charter_proto::GrantLearning {
            v: 1,
            apps: vec![charter_proto::LearningApp {
                id: "khan-academy".into(),
                label: "Khan Academy".into(),
                kind: charter_proto::LearningAppKind::Site,
                domains: vec!["khanacademy.org".into()],
                url: Some("https://www.khanacademy.org/".into()),
                exec: None,
                trusted: false,
                free: None,
            }],
            cap_minutes,
            paused,
            issued_at: 1,
        });
        p
    }

    #[test]
    fn learning_tick_does_not_reduce_remaining() {
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(OLDER, learning_child(None, None))], now, |_| {
            (None, None)
        });
        e.tick_bucket(Some(OLDER), now, 1800, Bucket::Learning);
        // Half an hour of maths: the 120-min screen budget is untouched…
        assert_eq!(e.used_secs(OLDER, now), 120 * 60);
        // …and the learning meter shows it.
        assert_eq!(e.learning_today(OLDER, now), Some(1800));
    }

    #[test]
    fn learning_over_cap_charges_screen() {
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        // 10-minute learning cap.
        e.sync(&[(OLDER, learning_child(Some(10), None))], now, |_| {
            (None, None)
        });
        e.tick_bucket(Some(OLDER), now, 600, Bucket::Learning);
        assert_eq!(e.used_secs(OLDER, now), 120 * 60, "under cap: screen free");
        // Over the cap: learning time falls back to costing screen time.
        e.tick_bucket(Some(OLDER), now, 300, Bucket::Learning);
        assert_eq!(e.learning_today(OLDER, now), Some(600), "cap holds");
        assert_eq!(e.used_secs(OLDER, now), 120 * 60 - 300, "screen charged");
    }

    #[test]
    fn learning_bucket_without_clause_charges_screen() {
        // Fail-closed: an attributed Learning tick with no learning clause in
        // force (none, or paused) must cost screen time, never be free.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(
            &[
                (YOUNGER, child("18:00", 60)),
                (OLDER, learning_child(None, Some(true))),
            ],
            now,
            |_| (None, None),
        );
        e.tick_bucket(Some(YOUNGER), now, 300, Bucket::Learning);
        assert_eq!(e.used_secs(YOUNGER, now), 60 * 60 - 300, "no clause");
        e.tick_bucket(Some(OLDER), now, 300, Bucket::Learning);
        assert_eq!(e.used_secs(OLDER, now), 120 * 60 - 300, "paused clause");
        assert_eq!(e.learning_today(OLDER, now), Some(0));
    }

    #[test]
    fn only_the_logged_in_child_accrues_time() {
        let now = wed_at(10); // mid-morning, both within their windows
        let mut e = two_kids(now);
        // Older is logged in for 30 minutes.
        e.tick(Some(OLDER), now, 1800);
        // Younger's budget is untouched; older's dropped by ~30 min.
        let younger_left = e.used_secs(YOUNGER, now);
        let older_left = e.used_secs(OLDER, now);
        assert_eq!(younger_left, 60 * 60, "younger spent nothing");
        // 120-min cap minus ~30 used ≈ 90 min (5400s) left.
        assert!(
            (5300..=5500).contains(&older_left),
            "older spent ~30 of their 120 min, got {older_left}s left"
        );
    }

    #[test]
    fn switching_login_charges_the_new_child() {
        let now = wed_at(10);
        let mut e = two_kids(now);
        e.tick(Some(YOUNGER), now, 600); // younger uses 10 min
        e.tick(Some(OLDER), now, 600); // sibling switches; older uses 10 min
        let younger_left = e.used_secs(YOUNGER, now);
        let older_left = e.used_secs(OLDER, now);
        assert!((2900..=3100).contains(&younger_left), "younger spent ~10m");
        assert!((6600..=6800).contains(&older_left), "older spent ~10m");
    }

    #[test]
    fn a_child_over_their_daily_cap_locks_independently() {
        let now = wed_at(10);
        let mut e = two_kids(now);
        // Younger (60 min cap) is logged in for 61 minutes.
        e.tick(Some(YOUNGER), now, 61 * 60);
        let d = e.tick(Some(YOUNGER), now, 0);
        assert!(dec(&d, YOUNGER).locked, "younger blew their 60-min cap");
        assert!(!dec(&d, OLDER).locked, "older is unaffected");
    }

    #[test]
    fn nobody_logged_in_charges_nobody() {
        let now = wed_at(10);
        let mut e = two_kids(now);
        e.tick(None, now, 1800);
        assert_eq!(e.used_secs(YOUNGER, now), 60 * 60);
        assert_eq!(e.used_secs(OLDER, now), 120 * 60);
    }

    #[test]
    fn sync_adds_relimits_and_drops_children() {
        let now = wed_at(10);
        let mut e = two_kids(now);
        assert_eq!(e.uids(), vec![YOUNGER, OLDER]);
        // Drop the older child, re-limit the younger.
        e.sync(&[(YOUNGER, child("19:00", 90))], now, |_| (None, None));
        assert_eq!(e.uids(), vec![YOUNGER]);
        // Re-limit kept them tracked; the new curfew (19:00) now applies at 18:30.
        let later = wed_at(18) + 1800;
        let d = e.tick(Some(YOUNGER), later, 0);
        assert!(!dec(&d, YOUNGER).locked, "younger's curfew moved to 19:00");
    }

    #[test]
    fn each_child_decision_carries_its_policy_source() {
        // Younger is guardian-governed; older is device-only — Signet-first mix.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(
            &[
                (YOUNGER, guardian_child("18:00", 60)),
                (OLDER, child("20:00", 120)),
            ],
            now,
            |_| (None, None),
        );
        let d = e.tick(Some(OLDER), now, 0);
        assert_eq!(dec(&d, YOUNGER).source, PolicySource::Guardian);
        assert_eq!(dec(&d, OLDER).source, PolicySource::DeviceOnly);
    }

    #[test]
    fn guardian_sourced_child_enforces_its_budget() {
        // A guardian-sourced 30-min cap is enforced exactly like a device-only
        // one — MultiChildEnforcer enforces whatever the EffectivePolicy carries.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, guardian_child("20:00", 30))], now, |_| {
            (None, None)
        });
        e.tick(Some(YOUNGER), now, 31 * 60); // 31 min against a 30-min cap
        let d = e.tick(Some(YOUNGER), now, 0);
        assert!(
            dec(&d, YOUNGER).locked,
            "guardian's 30-min cap locks the child"
        );
        assert_eq!(dec(&d, YOUNGER).source, PolicySource::Guardian);
    }

    #[test]
    fn remaining_reports_each_childs_own_time_and_none_for_untracked() {
        // The status surface (`charter status`) must report the CALLER's own
        // remaining time — each child sees their own on a shared box, and an
        // untracked uid gets None (the handler fail-safes that to "unknown").
        let now = wed_at(10);
        let mut e = two_kids(now); // younger 60m cap, older 120m cap
        e.tick(Some(YOUNGER), now, 15 * 60); // younger uses 15 min
        let y = e.remaining(YOUNGER, now).expect("younger tracked");
        let o = e.remaining(OLDER, now).expect("older tracked");
        assert!(
            (2600..=2800).contains(&y.budget_secs),
            "younger has ~45m of their 60m left, got {}s",
            y.budget_secs
        );
        assert_eq!(o.budget_secs, 120 * 60, "older spent nothing");
        assert!(e.remaining(9999, now).is_none(), "untracked uid -> None");
    }

    #[test]
    fn time_extend_unlocks_an_over_cap_child_on_the_enforcing_ledger() {
        // The whole point of #7: a guardian-approved time.extend must extend the
        // ledger the enforce loop actually reads (MultiChildEnforcer's per-child
        // ExtensionLedger), so an over-cap child is UNLOCKED — not merely have the
        // D-Bus readout move while enforcement keeps them frozen.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, guardian_child("20:00", 30))], now, |_| {
            (None, None)
        });
        e.tick(Some(YOUNGER), now, 31 * 60); // blow the 30-min cap
        assert!(
            dec(&e.tick(Some(YOUNGER), now, 0), YOUNGER).locked,
            "over the cap -> locked before any extension"
        );

        // Guardian approves +15 min on the budget dimension.
        assert!(
            e.apply_extension(YOUNGER, now, "req-1", 15, Dimension::Budget),
            "first application is newly applied"
        );
        assert!(
            !dec(&e.tick(Some(YOUNGER), now, 0), YOUNGER).locked,
            "the extension unlocks the previously-over-cap child"
        );

        // Idempotent by reqId; unknown uid is a no-op.
        assert!(
            !e.apply_extension(YOUNGER, now, "req-1", 15, Dimension::Budget),
            "the same reqId does not double-extend"
        );
        assert!(
            !e.apply_extension(9999, now, "req-2", 15, Dimension::Budget),
            "an untracked uid is a no-op"
        );
    }

    #[test]
    fn relimit_from_device_only_to_guardian_keeps_accrued_time() {
        // Child starts device-only, accrues 20 min, then the guardian adopts them
        // (same numbers). The accrued usage survives the source flip.
        let now = wed_at(10);
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, child("20:00", 60))], now, |_| (None, None));
        e.tick(Some(YOUNGER), now, 20 * 60); // spend 20 of 60 min
                                             // Guardian now governs the child (re-limit, same cap).
        e.sync(&[(YOUNGER, guardian_child("20:00", 60))], now, |_| {
            (None, None)
        });
        let left = e.used_secs(YOUNGER, now);
        assert!(
            (2300..=2500).contains(&left),
            "accrued ~20m must survive the source flip, got {left}s left"
        );
        let d = e.tick(Some(YOUNGER), now, 0);
        assert_eq!(dec(&d, YOUNGER).source, PolicySource::Guardian);
    }

    /// A buckets-only family (no budget clause at all) chooses `weekStart:
    /// sun` in the `buckets` clause — that must actually govern the week
    /// roll, not silently default to Monday because nothing read it.
    #[test]
    fn a_buckets_only_child_rolls_the_week_on_the_buckets_clauses_own_week_start() {
        const SAT_NOON: i64 = 1_704_499_200 + 12 * 3600; // Sat 2024-01-06 12:00 UTC
        const SUN_0000: i64 = 1_704_585_600; // Sun 2024-01-07 00:00 UTC (a NEW
                                             // week under weekStart:sun, but
                                             // still mid-week under Monday)

        let policy = EffectivePolicy {
            schedule: None,
            budget: None,
            learning: None,
            source: PolicySource::DeviceOnly,
        };
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, policy)], SAT_NOON, |_| (None, None));
        // No budget clause — set the buckets clause's own tz/weekStart
        // BEFORE the first tick, exactly as the runtime loop will.
        e.set_buckets_policy(YOUNGER, Some("UTC"), Some(WeekStart::Sun));

        e.tick_attributed(
            Some(YOUNGER),
            SAT_NOON,
            20 * 60,
            Bucket::Screen,
            Some("play"),
            false,
        );
        assert_eq!(e.app_bucket_week(YOUNGER, "play", SAT_NOON), Some(20 * 60));

        // Sunday 00:00 is a fresh week under weekStart:sun — the meter must
        // already have rolled. A Monday fallback would instead keep last
        // week's 20m alive until the FOLLOWING Monday.
        e.tick_attributed(Some(YOUNGER), SUN_0000, 0, Bucket::Screen, None, false);
        assert_eq!(e.app_bucket_week(YOUNGER, "play", SUN_0000), Some(0));
    }

    /// §2.3: `unrecognised` is credited ALONGSIDE the ordinary screen credit
    /// (never a separate pool), only for the ACTIVE child, and only when the
    /// caller actually says so.
    #[test]
    fn unrecognised_time_is_attributed_to_the_active_child_only() {
        let policy = EffectivePolicy {
            schedule: None,
            budget: None,
            learning: None,
            source: PolicySource::DeviceOnly,
        };
        const NOW: i64 = 1_704_499_200; // 2024-01-06 12:00 UTC
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(YOUNGER, policy.clone()), (OLDER, policy)], NOW, |_| {
            (None, None)
        });

        e.tick_attributed(Some(YOUNGER), NOW, 90, Bucket::Screen, None, true);
        assert_eq!(e.unrecognised_today(YOUNGER, NOW), Some(90));
        assert_eq!(e.used_today(YOUNGER, NOW), Some(90)); // screen still credited too
                                                          // The child who wasn't active this tick accrues nothing.
        assert_eq!(e.unrecognised_today(OLDER, NOW), Some(0));

        // `false` (an installed-but-ungrouped app, or a recognised identity)
        // must add nothing further.
        e.tick_attributed(Some(YOUNGER), NOW, 30, Bucket::Screen, None, false);
        assert_eq!(e.unrecognised_today(YOUNGER, NOW), Some(90));
    }

    // ---- The named-costs time model ---------------------------------------

    mod named_model {
        use super::*;

        const NOW: i64 = 1_704_499_200; // 2024-01-06 12:00 UTC

        /// A child on the named model with a Play allowance. The budget's
        /// numbers are generous — these tests are about WHAT is charged, never
        /// about locking.
        fn named_child() -> EffectivePolicy {
            let mut p = child("23:00", 600);
            let mut b = p.budget.take().expect("child() sets a budget");
            b.model = Some(charter_schedule::TimeModel::Named);
            p.budget = Some(b);
            p
        }

        fn enforcer(policy: EffectivePolicy) -> MultiChildEnforcer {
            let mut e = MultiChildEnforcer::new();
            e.sync(&[(YOUNGER, policy)], NOW, |_| (None, None));
            e
        }

        /// THE HEADLINE. decented's actual scene: two monitors, a game
        /// full-screen on one, YouTube on the other, and a maths window in
        /// front. Under the session model the focused window waived the lot
        /// and the day read zero. Here nothing is waived and nothing is
        /// doubled — the two costing windows spend the wall clock ONCE.
        #[test]
        fn two_costing_things_open_at_once_spend_the_clock_once() {
            let mut e = enforcer(named_child());
            let open = vec!["play".to_string(), "video".to_string()];
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, Some(&open), false);
            // The day: sixty seconds of wall clock, not a hundred and twenty.
            assert_eq!(e.used_today(YOUNGER, NOW), Some(60));
            // And each allowance sees its own full minute.
            assert_eq!(e.app_bucket_today(YOUNGER, "play", NOW), Some(60));
            assert_eq!(e.app_bucket_today(YOUNGER, "video", NOW), Some(60));
        }

        /// The whole inversion, in one assertion: being at the device is free.
        /// Under the session model this same tick charges the full sixty
        /// seconds.
        #[test]
        fn an_idle_desktop_costs_nothing() {
            let mut e = enforcer(named_child());
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, Some(&[]), false);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(0));
        }

        /// The other half of the same distinction, and the bug it closes: a
        /// display that could not be read is NOT an idle desktop. `Named` gates
        /// every one of its caps on seeing what is open, so a probe that can
        /// never succeed — a Wayland seat, a killed X server — used to mean
        /// zero against the daily cap, zero against every bucket, forever.
        /// `None` charges the Session baseline instead: no named allowance is
        /// spent on a guess, but the day does run down.
        #[test]
        fn a_display_that_cannot_be_read_charges_the_screen_baseline() {
            let mut e = enforcer(named_child());
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, None, false);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(60));
            // The baseline only. Naming an allowance would be a claim about a
            // specific app, made with no evidence about any app at all.
            assert_eq!(e.app_bucket_today(YOUNGER, "play", NOW), Some(0));
            // And a Learning attribution buys nothing here either — there is
            // no waiver in this model, unreadable display or not.
            let mut e = enforcer(named_child());
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Learning, None, false);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(60));
            assert_eq!(e.learning_today(YOUNGER, NOW), Some(0));
        }

        /// A free app is free by ABSENCE, not by a grant — which is the point.
        /// Khan open on its own names no allowance, so nothing is charged, and
        /// there is no waiver anywhere for a forged window to aim at.
        #[test]
        fn an_app_the_guardian_never_named_is_free_without_any_grant() {
            let mut e = enforcer(named_child());
            // `Bucket::Learning` is not even consulted in this model; pass the
            // ordinary Screen attribution to prove the result does not
            // depend on the waiver having been computed at all.
            e.tick_open(Some(YOUNGER), NOW, 300, Bucket::Screen, Some(&[]), false);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(0));
            // Nothing was waived either — there is no learning meter running
            // in this model, so free time leaves no trace to have to defend.
            assert_eq!(e.learning_today(YOUNGER, NOW), Some(0));
        }

        /// Closing the costing thing stops the clock; opening it starts it.
        /// The sentence a nine-year-old can act on, asserted.
        #[test]
        fn the_clock_runs_only_while_something_costing_is_open() {
            let mut e = enforcer(named_child());
            let play = vec!["play".to_string()];
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, Some(&play), false);
            e.tick_open(
                Some(YOUNGER),
                NOW + 60,
                60,
                Bucket::Screen,
                Some(&[]),
                false,
            );
            e.tick_open(
                Some(YOUNGER),
                NOW + 120,
                60,
                Bucket::Screen,
                Some(&play),
                false,
            );
            assert_eq!(e.used_today(YOUNGER, NOW), Some(120));
            assert_eq!(e.app_bucket_today(YOUNGER, "play", NOW), Some(120));
        }

        /// §4.4 — the journal is what the cross-device union pools, so a
        /// minute in which nothing cost must not be marked. Otherwise free
        /// time on a laptop silently consumes a phone's allowance, and the
        /// over-charge arrives from a device the family wasn't even using.
        #[test]
        fn the_minute_journal_only_marks_minutes_that_cost() {
            let mut e = enforcer(named_child());
            // A full minute with nothing costing open: the journal stays
            // EMPTY, which is what STATUS omits rather than publishing as a
            // spent minute for another device to pool against.
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, Some(&[]), false);
            assert_eq!(e.minutes_today_b64(YOUNGER, NOW), None);
            // The next minute, with something open, does mark.
            let play = vec!["play".to_string()];
            e.tick_open(
                Some(YOUNGER),
                NOW + 60,
                60,
                Bucket::Screen,
                Some(&play),
                false,
            );
            assert!(e.minutes_today_b64(YOUNGER, NOW).is_some());
        }

        /// The coverage signal must survive precisely when it matters. Named
        /// makes an out-of-date cost list the system's characteristic failure,
        /// so a counter that went quiet whenever nothing was charged would
        /// report a complete list at the exact moment it was least true.
        #[test]
        fn unrecognised_time_is_still_reported_when_nothing_was_charged() {
            let mut e = enforcer(named_child());
            e.tick_open(Some(YOUNGER), NOW, 90, Bucket::Screen, Some(&[]), true);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(0));
            assert_eq!(e.unrecognised_today(YOUNGER, NOW), Some(90));
        }

        /// Back-compat, asserted on the wire shape rather than the struct
        /// default: a budget clause signed before `model` existed still
        /// charges for being at the device, waiver and all.
        #[test]
        fn a_budget_signed_before_the_model_field_charges_the_baseline() {
            let json = r#"{"v":1,"tz":"UTC","dailyMinutes":120,"issuedAt":1}"#;
            let b: charter_schedule::GrantBudget =
                serde_json::from_str(json).expect("legacy budget parses");
            assert_eq!(
                charter_schedule::TimeModel::of(Some(&b)),
                charter_schedule::TimeModel::Session
            );
            let mut p = child("23:00", 120);
            p.budget = Some(b);
            let mut e = enforcer(p);
            // Nothing costing open, and it still charges — the old meaning,
            // untouched.
            e.tick_open(Some(YOUNGER), NOW, 60, Bucket::Screen, Some(&[]), false);
            assert_eq!(e.used_today(YOUNGER, NOW), Some(60));
        }

        /// `tick_attributed` is the one-window shim over `tick_open`; the two
        /// must not drift, or Android (one foreground app) and Linux (a window
        /// set) would meter differently for the same charter.
        #[test]
        fn the_single_window_shim_agrees_with_the_open_set() {
            let mut a = enforcer(named_child());
            let mut b = enforcer(named_child());
            a.tick_attributed(Some(YOUNGER), NOW, 60, Bucket::Screen, Some("play"), false);
            b.tick_open(
                Some(YOUNGER),
                NOW,
                60,
                Bucket::Screen,
                Some(&["play".to_string()]),
                false,
            );
            assert_eq!(a.used_today(YOUNGER, NOW), b.used_today(YOUNGER, NOW));
            assert_eq!(
                a.app_bucket_today(YOUNGER, "play", NOW),
                b.app_bucket_today(YOUNGER, "play", NOW)
            );
        }
    }
}
