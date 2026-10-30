//! The single canonical today-only additive extension ledger. Entries are
//! idempotent by reqId, reset at the local-tz day roll, and pooled per
//! dimension (schedule vs budget). Phase 7's enactor only *pushes* entries here
//! — it never redefines the enforcer math.

use chrono::DateTime;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Which limit an extension applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dimension {
    Schedule,
    Budget,
}

/// One named time's extension pool (§Times).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TimePool {
    /// Seconds granted today.
    secs: u64,
    /// Wall-clock seconds of `secs` already SPENT while the device was running
    /// this time off the pool (see [`ExtensionLedger::burn_time`]).
    #[serde(default)]
    consumed: u64,
    /// Top-up order: the ledger's `time_topup_seq` at this pool's latest
    /// grant. A counter rather than a clock reading, so two grants within one
    /// second still order, and a clock that stepped back cannot reorder them.
    #[serde(default)]
    seq: u64,
}

/// Today-only additive extension pools.
///
/// # Pools and the fall-through order (§Times)
///
/// There are four kinds of pool. The whole-device `schedule` and `budget`
/// pools are FREE time's. Bucket pools top up one named app bucket. **Time
/// pools** (`apply_time`) top up a named time by id.
///
/// Inside a window that can run, the window decides and NO pool burns. When
/// the window's own time cannot run — the device is outside every window, or
/// the window's time is Free (or a `countsAgainstBudget` named time) and the
/// budget is spent — the warden falls through the pools in this order:
///
/// 1. Free's pool for the dimension that is locking: the schedule pool when
///    outside every window, the budget pool when budget-locked (today's
///    routing, unchanged).
/// 2. Otherwise the named pool most recently topped up that still has minutes
///    ([`most_recent_named_pool_with_minutes`](Self::most_recent_named_pool_with_minutes)),
///    which the device then runs AS THAT NAMED TIME.
/// 3. Nothing left: locked.
///
/// The ledger only stores and orders the pools; the warden implements the
/// routing, and burns the pool it is running off with
/// [`burn_schedule`](Self::burn_schedule) / [`burn_time`](Self::burn_time) —
/// wall clock, never usage, so an idle ward cannot stretch a pool. A pool-run
/// span ends when the pool does. A `time` riding on a bucket-hit grant is
/// ignored: bucket routing wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionLedger {
    tz: String,
    day_key: String,
    applied: Vec<String>, // reqId hexes already applied (idempotency)
    schedule_secs: u64,
    budget_secs: u64,
    /// Wall-clock seconds of the schedule pool already SPENT while the window
    /// was closed. The budget pool self-burns (its base is `cap − used`); the
    /// schedule pool has no counterweight — without this an out-of-window
    /// "+30 minutes" never expires: frozen at 30:00, unlocked to midnight
    /// (the on-device fail-open, 2026-07-24). `#[serde(default)]` so older
    /// snapshots restore unburned (they carry no consumption to preserve).
    #[serde(default)]
    schedule_consumed_secs: u64,
    /// Usage at the moment the day's FIRST budget grant landed — the floor the
    /// grant is measured from.
    ///
    /// The budget pool normally self-burns because its base is `cap - used`.
    /// That breaks for a ward who is already OVER the cap: the base is pinned
    /// at zero, so a grant is swallowed by the existing overdraft and the
    /// guardian's "give 30 minutes" silently buys nothing (decented, 2026-07-31,
    /// with a ward 2h43m over). Recording where the debt stood lets a grant
    /// mean what it says — 30 minutes of real screen time — WITHOUT forgiving
    /// the overdraft, which stays visible and still resets at midnight.
    #[serde(default)]
    budget_baseline_today_secs: Option<u64>,
    #[serde(default)]
    budget_baseline_week_secs: Option<u64>,
    /// Today-only SUBTRACTIVE pools — the guardian taking time back ("you've
    /// had twenty minutes off today"), the mirror of the additive pools above.
    ///
    /// A deduction lives here rather than being charged to the `UsageLedger`
    /// because usage is a FACTUAL record of what the ward actually did: it
    /// feeds the weekly picture and the cross-device usage sync, and inventing
    /// twenty minutes of screen time they never had would corrupt both. The
    /// allowance is the thing the guardian is entitled to change, so the
    /// allowance is what moves. Same day-scoping and reqId idempotency as a
    /// grant, so "take 20" applied twice by a re-read of the same record is
    /// still 20. `#[serde(default)]` so older snapshots restore undeducted.
    #[serde(default)]
    schedule_deduct_secs: u64,
    #[serde(default)]
    budget_deduct_secs: u64,
    /// Today-only additive pools keyed by bucket/group id — the named-times
    /// pool an ask or a gift can target instead of the whole-device schedule
    /// or budget dimension. `#[serde(default)]` so pre-existing snapshots
    /// (recorded before this field existed) restore with an empty map rather
    /// than failing to parse.
    #[serde(default)]
    bucket_extra: std::collections::BTreeMap<String, u64>,
    /// Today-only additive pools keyed by NAMED-TIME id (§Times). A separate
    /// map from `bucket_extra`: a bucket and a time may legitimately share an
    /// id, and must not share minutes. `#[serde(default)]` so older snapshots
    /// restore with no time pools.
    #[serde(default)]
    time_pools: std::collections::BTreeMap<String, TimePool>,
    /// Monotonic top-up counter ordering the time pools; see [`TimePool::seq`].
    #[serde(default)]
    time_topup_seq: u64,
}

/// The `YYYY-MM-DD` calendar-date key for `now_unix` in `tz` — the daily reset
/// boundary shared by the extension pool, the usage reset, and the STATUS feed.
pub fn day_key(tz: Tz, now_unix: i64) -> String {
    DateTime::from_timestamp(now_unix, 0)
        .expect("valid timestamp")
        .with_timezone(&tz)
        .format("%Y-%m-%d")
        .to_string()
}

impl ExtensionLedger {
    /// A fresh ledger for `tz`.
    pub fn new(tz: &str, now_unix: i64) -> Self {
        let parsed: Tz = tz.parse().unwrap_or(chrono_tz::UTC);
        ExtensionLedger {
            tz: tz.to_string(),
            day_key: day_key(parsed, now_unix),
            applied: Vec::new(),
            schedule_secs: 0,
            budget_secs: 0,
            schedule_consumed_secs: 0,
            budget_baseline_today_secs: None,
            budget_baseline_week_secs: None,
            schedule_deduct_secs: 0,
            budget_deduct_secs: 0,
            bucket_extra: std::collections::BTreeMap::new(),
            time_pools: std::collections::BTreeMap::new(),
            time_topup_seq: 0,
        }
    }

    fn tz(&self) -> Tz {
        self.tz.parse().unwrap_or(chrono_tz::UTC)
    }

    /// Re-key the day-roll tz to match the enforcement tz (e.g. the guardian
    /// edited the clause tz) WITHOUT discarding today's applied extensions — a
    /// today-only extension must roll on the SAME tz the enforcer keys its day
    /// off, else a granted extension expires early or lingers past midnight.
    pub fn reconcile(&mut self, tz: &str, now_unix: i64) {
        if self.tz == tz {
            return;
        }
        // Roll in the OLD tz first. Re-keying stamps `day_key` with today, and
        // every getter trusts that stamp — so without this, a ledger whose
        // pools belong to a PAST day (a snapshot restored after a restart,
        // now that they are persisted) had them re-dated to today the moment
        // the clause tz differed: yesterday's "+30" live again. Pools that
        // really are today's survive the roll and are re-keyed as before.
        self.roll(now_unix);
        self.tz = tz.to_string();
        self.day_key = day_key(self.tz(), now_unix);
    }

    /// Whether `a` and `b` fall in the same local day, by THIS ledger's tz —
    /// the very boundary [`roll`](Self::roll) uses to clear the applied list.
    ///
    /// Exposed because a caller that re-reads a durable record every tick has
    /// to ask the question, and the only wrong answer is a second definition
    /// of "day". The ledger's idempotency is an applied-id list that the day
    /// roll CLEARS; an id from yesterday is therefore not refused, it is
    /// simply unknown again — so anything replaying an old record must filter
    /// by the same boundary the roll uses, or the roll hands it a fresh
    /// licence every midnight.
    pub fn same_day(&self, a: i64, b: i64) -> bool {
        let tz = self.tz();
        day_key(tz, a) == day_key(tz, b)
    }

    fn roll(&mut self, now_unix: i64) {
        let dk = day_key(self.tz(), now_unix);
        if dk != self.day_key {
            self.day_key = dk;
            self.applied.clear();
            self.schedule_secs = 0;
            self.budget_secs = 0;
            self.schedule_consumed_secs = 0;
            self.budget_baseline_today_secs = None;
            self.budget_baseline_week_secs = None;
            self.schedule_deduct_secs = 0;
            self.budget_deduct_secs = 0;
            self.bucket_extra.clear();
            self.time_pools.clear();
            self.time_topup_seq = 0;
        }
    }

    /// Burn `elapsed_secs` of wall clock off the schedule pool. The enforcing
    /// tick calls this ONLY while the schedule window is CLOSED (the pool is
    /// what's keeping the device unlocked, so it must count down); while the
    /// window is open the pool is untouched — "+30 minutes" granted in-window
    /// still means 30 minutes past the close. Saturating; day-scoped via roll.
    pub fn burn_schedule(&mut self, now_unix: i64, elapsed_secs: u64) {
        self.roll(now_unix);
        if self.schedule_consumed_secs < self.schedule_secs {
            self.schedule_consumed_secs = self
                .schedule_consumed_secs
                .saturating_add(elapsed_secs)
                .min(self.schedule_secs);
        }
    }

    /// Apply a `minutes` extension for `req_id` to `dim`. Idempotent by reqId,
    /// today-only. Returns true if newly applied.
    pub fn apply(&mut self, now_unix: i64, req_id: &str, minutes: u16, dim: Dimension) -> bool {
        self.roll(now_unix);
        if self.applied.iter().any(|r| r == req_id) {
            return false;
        }
        self.applied.push(req_id.to_string());
        let secs = minutes as u64 * 60;
        match dim {
            Dimension::Schedule => self.schedule_secs += secs,
            Dimension::Budget => self.budget_secs += secs,
        }
        true
    }

    /// Apply a `minutes` extension for `req_id` to the named `bucket_id`'s
    /// pool. Idempotent by `req_id`, today-only — the per-group mirror of
    /// [`apply`](Self::apply).
    ///
    /// Shares the SAME `applied` list as `apply` and `deduct`: one reqId can
    /// land exactly once, whether it targets a whole-device dimension or a
    /// named bucket, so a grant can never be double-counted across the two
    /// pool kinds. Returns true if newly applied.
    pub fn apply_bucket(
        &mut self,
        now_unix: i64,
        req_id: &str,
        minutes: u16,
        bucket_id: &str,
    ) -> bool {
        self.roll(now_unix);
        if self.applied.iter().any(|r| r == req_id) {
            return false;
        }
        self.applied.push(req_id.to_string());
        let secs = minutes as u64 * 60;
        *self.bucket_extra.entry(bucket_id.to_string()).or_insert(0) += secs;
        true
    }

    /// Extra seconds pooled today for `bucket_id` (0 after the day rolls or
    /// if the bucket has never received a grant).
    pub fn bucket_extra_secs(&self, now_unix: i64, bucket_id: &str) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            0
        } else {
            self.bucket_extra.get(bucket_id).copied().unwrap_or(0)
        }
    }

    /// Apply a `minutes` extension for `req_id` to the named time `time_id`'s
    /// pool (§Times). Idempotent by `req_id`, today-only — the per-time mirror
    /// of [`apply`](Self::apply).
    ///
    /// Shares the ONE `applied` list with every other pool kind, so a reqId
    /// lands exactly once whichever pool it targets. The id is stored as given:
    /// whether it names a time this device knows is the warden's decision at
    /// routing time (an unknown id is never run — see
    /// [`most_recent_named_pool_among`](Self::most_recent_named_pool_among)),
    /// so a grant for a time that is defined LATER today is not lost. Returns
    /// true if newly applied.
    pub fn apply_time(&mut self, now_unix: i64, req_id: &str, minutes: u16, time_id: &str) -> bool {
        self.roll(now_unix);
        if self.applied.iter().any(|r| r == req_id) {
            return false;
        }
        self.applied.push(req_id.to_string());
        self.time_topup_seq += 1;
        let seq = self.time_topup_seq;
        let pool = self.time_pools.entry(time_id.to_string()).or_default();
        pool.secs = pool.secs.saturating_add(minutes as u64 * 60);
        pool.seq = seq;
        true
    }

    /// Extra seconds still unspent in `time_id`'s pool (0 after the day rolls,
    /// or if the time has never received a grant).
    pub fn time_extra_secs(&self, now_unix: i64, time_id: &str) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            return 0;
        }
        self.time_pools
            .get(time_id)
            .map(|p| p.secs.saturating_sub(p.consumed))
            .unwrap_or(0)
    }

    /// Burn `elapsed_secs` of wall clock off `time_id`'s pool. The enforcing
    /// tick calls this ONLY while the device is running that time off its pool
    /// (outside any window able to run it); while a window decides, the pool is
    /// untouched. Saturating; day-scoped via roll — the per-time twin of
    /// [`burn_schedule`](Self::burn_schedule), and for the same reason: an
    /// out-of-window "+30 minutes" that never counts down is unlocked to
    /// midnight.
    pub fn burn_time(&mut self, now_unix: i64, time_id: &str, elapsed_secs: u64) {
        self.roll(now_unix);
        if let Some(p) = self.time_pools.get_mut(time_id) {
            p.consumed = p.consumed.saturating_add(elapsed_secs).min(p.secs);
        }
    }

    /// The named time whose pool was most recently topped up and still has
    /// minutes — step 2 of the fall-through order in the type docs. `None` if
    /// no named pool has time left (or the day rolled).
    ///
    /// Takes every pool at its word; a warden that knows the child's `times`
    /// clause should use [`most_recent_named_pool_among`](Self::most_recent_named_pool_among)
    /// so a pool for an id it does not know can never be picked.
    pub fn most_recent_named_pool_with_minutes(&self, now_unix: i64) -> Option<String> {
        self.most_recent_named_pool_among(now_unix, |_| true)
    }

    /// As [`most_recent_named_pool_with_minutes`](Self::most_recent_named_pool_with_minutes),
    /// restricted to ids for which `known` holds. Pools for other ids are
    /// skipped, not discarded. An unknown time's minutes enact nothing — a
    /// loosening with an unenforceable posture fails safe — but are kept, so a
    /// `times` clause that arrives later the same day still honours them.
    pub fn most_recent_named_pool_among(
        &self,
        now_unix: i64,
        known: impl Fn(&str) -> bool,
    ) -> Option<String> {
        if day_key(self.tz(), now_unix) != self.day_key {
            return None;
        }
        self.time_pools
            .iter()
            .filter(|(id, p)| p.secs > p.consumed && known(id))
            .max_by_key(|(_, p)| p.seq)
            .map(|(id, _)| id.clone())
    }

    /// Take `minutes` back off today's allowance for `dim`. Idempotent by
    /// `req_id`, today-only, saturating — the mirror of [`apply`](Self::apply).
    ///
    /// Shares ONE `applied` list with grants so a single id can never be both
    /// granted and deducted, and so a record re-read from disk on the next tick
    /// (which is exactly how the local-adjustment file is consumed) applies
    /// once. Returns true if newly applied.
    pub fn deduct(&mut self, now_unix: i64, req_id: &str, minutes: u16, dim: Dimension) -> bool {
        self.roll(now_unix);
        if self.applied.iter().any(|r| r == req_id) {
            return false;
        }
        self.applied.push(req_id.to_string());
        let secs = minutes as u64 * 60;
        match dim {
            Dimension::Schedule => {
                self.schedule_deduct_secs = self.schedule_deduct_secs.saturating_add(secs)
            }
            Dimension::Budget => {
                self.budget_deduct_secs = self.budget_deduct_secs.saturating_add(secs)
            }
        }
        true
    }

    /// Schedule seconds taken back today (0 after the day rolls).
    pub fn schedule_deducted_secs(&self, now_unix: i64) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            0
        } else {
            self.schedule_deduct_secs
        }
    }

    /// Budget seconds taken back today (0 after the day rolls).
    pub fn budget_deducted_secs(&self, now_unix: i64) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            0
        } else {
            self.budget_deduct_secs
        }
    }

    /// Record where the ward's usage stood when the day's first budget grant
    /// landed. Only the FIRST is kept: later grants stack on the same floor, so
    /// two grants of 15 give 30 usable minutes, not a moving goalpost.
    ///
    /// Call this only when `apply` reported a grant as newly applied.
    pub fn note_budget_baseline(&mut self, now_unix: i64, used_today: u64, used_week: u64) {
        self.roll(now_unix);
        if self.budget_baseline_today_secs.is_none() {
            self.budget_baseline_today_secs = Some(used_today);
            self.budget_baseline_week_secs = Some(used_week);
        }
    }

    /// The usage floor a budget grant is measured from, if one was recorded
    /// today: `(today, week)`. `None` before any grant, or after the day rolls.
    pub fn budget_baseline_secs(&self, now_unix: i64) -> Option<(u64, u64)> {
        if day_key(self.tz(), now_unix) != self.day_key {
            return None;
        }
        match (
            self.budget_baseline_today_secs,
            self.budget_baseline_week_secs,
        ) {
            (Some(t), Some(w)) => Some((t, w)),
            _ => None,
        }
    }

    /// Extra schedule seconds still unspent (0 after the day rolls).
    pub fn schedule_extra_secs(&self, now_unix: i64) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            0
        } else {
            self.schedule_secs
                .saturating_sub(self.schedule_consumed_secs)
        }
    }

    /// Extra budget seconds (0 after the day rolls).
    pub fn budget_extra_secs(&self, now_unix: i64) -> u64 {
        if day_key(self.tz(), now_unix) != self.day_key {
            0
        } else {
            self.budget_secs
        }
    }

    pub fn snapshot(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_snapshot(s: &str) -> Option<ExtensionLedger> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: &str = "Europe/London";
    const NOON: i64 = 1_782_734_400; // Mon 2026-06-29 12:00 BST

    /// Pools are persisted now, so a ledger can wake up holding YESTERDAY's.
    /// Every getter is day-guarded, so that is harmless — unless `reconcile`
    /// re-dates the stale pools to today, which a changed clause tz made it do.
    #[test]
    fn reconciling_a_stale_ledger_to_a_new_tz_does_not_revive_yesterdays_grant() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert!(l.apply(NOON, "req-a", 30, Dimension::Budget));
        let restored = ExtensionLedger::from_snapshot(&l.snapshot()).unwrap();

        let next_day = NOON + 86_400;
        let mut stale = restored.clone();
        stale.reconcile("America/New_York", next_day);
        assert_eq!(
            stale.budget_extra_secs(next_day),
            0,
            "yesterday's +30 stays dead"
        );

        // The case `reconcile` exists for: a tz edit the SAME day keeps it.
        let mut same_day = restored;
        same_day.reconcile("Europe/Paris", NOON + 60);
        assert_eq!(same_day.budget_extra_secs(NOON + 60), 30 * 60);
    }

    #[test]
    fn additive_and_idempotent_by_reqid() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert!(l.apply(NOON, "req-a", 15, Dimension::Budget));
        assert!(l.apply(NOON, "req-b", 10, Dimension::Budget));
        assert!(!l.apply(NOON, "req-a", 15, Dimension::Budget)); // replay
        assert_eq!(l.budget_extra_secs(NOON), 25 * 60);
    }

    #[test]
    fn dimension_isolation() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-s", 30, Dimension::Schedule);
        assert_eq!(l.schedule_extra_secs(NOON), 30 * 60);
        assert_eq!(l.budget_extra_secs(NOON), 0);
    }

    #[test]
    fn day_roll_discards() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-a", 30, Dimension::Budget);
        let next_day = NOON + 24 * 3600;
        assert_eq!(l.budget_extra_secs(next_day), 0);
        // After the roll a new extension applies fresh.
        assert!(l.apply(next_day, "req-c", 5, Dimension::Budget));
        assert_eq!(l.budget_extra_secs(next_day), 5 * 60);
    }

    // The out-of-window fail-open (found on-device 2026-07-24: an approved
    // 30m out-of-schedule extension froze at "30 mins left" and never
    // re-locked): the schedule pool must BURN with wall-clock time while the
    // window is closed — the budget pool self-burns via usage, the schedule
    // pool has no counterweight unless we give it one.

    #[test]
    fn schedule_pool_burns_with_wall_clock_when_burned() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-s", 30, Dimension::Schedule);
        assert_eq!(l.schedule_extra_secs(NOON), 30 * 60);
        // 10 minutes of out-of-window wall clock burn.
        l.burn_schedule(NOON + 600, 600);
        assert_eq!(l.schedule_extra_secs(NOON + 600), 20 * 60);
        // Burn past the pool: saturates at 0, never wraps.
        l.burn_schedule(NOON + 3600, 3000);
        assert_eq!(l.schedule_extra_secs(NOON + 3600), 0);
    }

    #[test]
    fn schedule_burn_is_day_scoped_and_stacking_extends() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-s", 30, Dimension::Schedule);
        l.burn_schedule(NOON + 25 * 60, 25 * 60);
        assert_eq!(l.schedule_extra_secs(NOON + 25 * 60), 5 * 60);
        // A second grant stacks on the UNBURNED remainder.
        l.apply(NOON + 25 * 60, "req-s2", 15, Dimension::Schedule);
        assert_eq!(l.schedule_extra_secs(NOON + 25 * 60), 20 * 60);
        // Day roll clears pool AND consumption.
        let next_day = NOON + 24 * 3600;
        assert_eq!(l.schedule_extra_secs(next_day), 0);
        l.apply(next_day, "req-s3", 10, Dimension::Schedule);
        assert_eq!(l.schedule_extra_secs(next_day), 10 * 60);
    }

    #[test]
    fn burn_snapshot_survives_restart_no_refill() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-s", 30, Dimension::Schedule);
        l.burn_schedule(NOON + 600, 600);
        let restored = ExtensionLedger::from_snapshot(&l.snapshot()).unwrap();
        assert_eq!(restored.schedule_extra_secs(NOON + 600), 20 * 60);
    }

    #[test]
    fn pre_burn_snapshot_restores_with_zero_consumed() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply(NOON, "req-s", 30, Dimension::Schedule);
        let mut v: serde_json::Value = serde_json::from_str(&l.snapshot()).unwrap();
        v.as_object_mut().unwrap().remove("schedule_consumed_secs");
        let restored = ExtensionLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(restored.schedule_extra_secs(NOON), 30 * 60);
    }

    #[test]
    fn bucket_pool_is_per_group_idempotent_and_today_only() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert!(l.apply_bucket(NOON, "req-p", 30, "play"));
        assert!(!l.apply_bucket(NOON, "req-p", 30, "play")); // replay
        l.apply_bucket(NOON, "req-s", 15, "social");
        assert_eq!(l.bucket_extra_secs(NOON, "play"), 30 * 60);
        assert_eq!(l.bucket_extra_secs(NOON, "social"), 15 * 60);
        assert_eq!(l.bucket_extra_secs(NOON, "reading"), 0);
        assert_eq!(l.bucket_extra_secs(NOON + 24 * 3600, "play"), 0); // day roll
    }

    #[test]
    fn one_reqid_cannot_be_both_device_and_bucket() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert!(l.apply(NOON, "req-x", 10, Dimension::Budget));
        assert!(!l.apply_bucket(NOON, "req-x", 10, "play"));
    }

    #[test]
    fn bucket_pool_survives_snapshot() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply_bucket(NOON, "req-p", 30, "play");
        let r = ExtensionLedger::from_snapshot(&l.snapshot()).unwrap();
        assert_eq!(r.bucket_extra_secs(NOON, "play"), 30 * 60);
    }

    #[test]
    fn time_pools_are_per_time_idempotent_shared_applied_and_today_only() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert!(l.apply_time(NOON, "req-a", 30, "learning"));
        assert!(!l.apply_time(NOON, "req-a", 30, "learning")); // replay
        assert!(!l.apply(NOON, "req-a", 30, Dimension::Budget)); // one id, one pool
        assert!(!l.apply_bucket(NOON, "req-a", 30, "learning"));
        assert!(l.apply_time(NOON, "req-b", 15, "learning"));
        assert!(l.apply_time(NOON, "req-c", 10, "chores"));
        assert_eq!(l.time_extra_secs(NOON, "learning"), 45 * 60);
        assert_eq!(l.time_extra_secs(NOON, "chores"), 10 * 60);
        assert_eq!(l.time_extra_secs(NOON, "other"), 0);
        // Free's pools and the bucket pools are untouched, and a bucket may
        // share an id with a time without sharing minutes.
        assert_eq!(l.budget_extra_secs(NOON), 0);
        assert_eq!(l.schedule_extra_secs(NOON), 0);
        assert_eq!(l.bucket_extra_secs(NOON, "learning"), 0);
        assert_eq!(l.time_extra_secs(NOON + 24 * 3600, "learning"), 0); // day roll
        assert_eq!(
            l.most_recent_named_pool_with_minutes(NOON + 24 * 3600),
            None
        );
    }

    #[test]
    fn time_pool_burns_with_wall_clock_and_saturates() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply_time(NOON, "req-a", 30, "learning");
        l.burn_time(NOON + 600, "learning", 600);
        assert_eq!(l.time_extra_secs(NOON + 600, "learning"), 20 * 60);
        l.burn_time(NOON + 4000, "learning", 99_999);
        assert_eq!(l.time_extra_secs(NOON + 4000, "learning"), 0);
        // Burning a pool that does not exist is a no-op, never a panic.
        l.burn_time(NOON + 4000, "nope", 60);
        // A later top-up stacks on the unburned remainder (here, nothing).
        l.apply_time(NOON + 4000, "req-b", 5, "learning");
        assert_eq!(l.time_extra_secs(NOON + 4000, "learning"), 5 * 60);
    }

    #[test]
    fn most_recent_named_pool_is_by_top_up_order_and_needs_minutes() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        assert_eq!(l.most_recent_named_pool_with_minutes(NOON), None);
        l.apply_time(NOON, "req-a", 30, "learning");
        l.apply_time(NOON, "req-b", 30, "chores"); // same second: counter orders
        assert_eq!(
            l.most_recent_named_pool_with_minutes(NOON).as_deref(),
            Some("chores")
        );
        // Spent pools drop out; the earlier one is next.
        l.burn_time(NOON + 1800, "chores", 1800);
        assert_eq!(
            l.most_recent_named_pool_with_minutes(NOON + 1800)
                .as_deref(),
            Some("learning")
        );
        // Topping learning up again makes it the most recent, and a restriction
        // to known ids skips an id the device has no definition for.
        l.apply_time(NOON + 1800, "req-c", 5, "mystery");
        assert_eq!(
            l.most_recent_named_pool_among(NOON + 1800, |id| id != "mystery")
                .as_deref(),
            Some("learning")
        );
        assert_eq!(
            l.most_recent_named_pool_with_minutes(NOON + 1800)
                .as_deref(),
            Some("mystery")
        );
    }

    #[test]
    fn time_pools_survive_snapshot_and_old_snapshots_restore_empty() {
        let mut l = ExtensionLedger::new(TZ, NOON);
        l.apply_time(NOON, "req-a", 30, "learning");
        l.burn_time(NOON + 60, "learning", 60);
        let r = ExtensionLedger::from_snapshot(&l.snapshot()).unwrap();
        assert_eq!(r.time_extra_secs(NOON + 60, "learning"), 29 * 60);
        assert_eq!(
            r.most_recent_named_pool_with_minutes(NOON + 60).as_deref(),
            Some("learning")
        );
        let mut v: serde_json::Value = serde_json::from_str(&l.snapshot()).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("time_pools");
        o.remove("time_topup_seq");
        let old = ExtensionLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(old.time_extra_secs(NOON, "learning"), 0);
    }
}
