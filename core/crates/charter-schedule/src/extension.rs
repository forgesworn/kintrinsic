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

/// Today-only additive extension pools.
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
}
