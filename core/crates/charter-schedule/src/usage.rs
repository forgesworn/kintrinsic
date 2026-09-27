//! Active-usage accounting. Credits **only** Active time (Idle / Locked /
//! Suspended / FrozenByCharter are excluded), resets per-tz at local midnight
//! (day) and at `weekStart` (week), and survives a restart via a snapshot that
//! **cannot refill the quota**.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::clause::WeekStart;
use crate::minutes::MinuteSet;

/// The managed session's activity state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Active,
    Idle,
    Locked,
    Suspended,
    FrozenByCharter,
}

impl Activity {
    /// Whether this state counts against the quota.
    pub fn counts(&self) -> bool {
        matches!(self, Activity::Active)
    }
}

/// Which meter an active tick feeds. `Screen` drains the child's budget
/// (the only pre-learning behaviour); `Learning` is tracked separately and
/// never drains it — guardian-designated learning apps are time-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Screen,
    Learning,
}

/// Durable usage accounting keyed to the schedule/budget tz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLedger {
    tz: String,
    week_start: WeekStart,
    day_key: String,
    week_key: String,
    used_today_secs: u64,
    used_week_secs: u64,
    /// Day-keyed learning-bucket seconds. `#[serde(default)]` so pre-learning
    /// snapshots restore with a zero counter (upgrades keep spent quota).
    #[serde(default)]
    learning_today_secs: u64,
    /// Day-keyed seconds whose resolved identity was ABSENT from the
    /// device's own inventory — software Charter does not know about at all
    /// (spec §2.3). Separate from `used_today_secs`: this is additional
    /// information about screen time already credited, never a different
    /// pool of time. `#[serde(default)]` so pre-§2.3 snapshots restore with a
    /// zero counter.
    #[serde(default)]
    unrecognised_today_secs: u64,
    /// Day-keyed seconds spent in an app the `alwaysavailable` clause opened
    /// while the device was LOCKED (spec 2026-08-03) — the 2am audiobook, the
    /// sleepover message. Like `unrecognised_today_secs` this is additional
    /// information ABOUT the day and never a different pool of time: it is
    /// deliberately absent from `used_today_secs`, because charging a ward for
    /// a bad night's sleep would make the counting and the enforcing disagree.
    ///
    /// One counter, not two: use after the daily limit is spent at 5pm counts
    /// the same as use at 2am. The lock reason is Charter's business; the
    /// pattern is the family's.
    ///
    /// `#[serde(default)]` so pre-2026-08-03 snapshots restore with a zero
    /// counter.
    #[serde(default)]
    out_of_hours_today_secs: u64,
    /// Week-keyed total, for the guardian's weekly line. Rolls with
    /// `used_week_secs`, not with the day.
    #[serde(default)]
    out_of_hours_week_secs: u64,
    /// Week-keyed count of DISTINCT DAYS that saw any out-of-hours use — the
    /// "3 nights" half of the line. Incremented exactly when the day counter
    /// crosses 0 → non-zero, so three wakings in one night are one night.
    #[serde(default)]
    out_of_hours_nights_week: u32,
    /// Day-keyed active-minutes journal (the union-rule unit — B3). Both
    /// buckets mark it: learning is time-FREE for the budget, but it is still
    /// screen time, and the weekly picture must not show a learning hour as
    /// "off screen". `#[serde(default)]` so pre-B3 snapshots restore empty.
    #[serde(default)]
    minutes_today: MinuteSet,
    /// Day-keyed meters for the guardian's named app buckets ("Play"), keyed by
    /// `AppBucket.id`. Separate from `learning_today_secs` because the two are
    /// opposites: learning time is FREE (never drains the day), whereas a
    /// capped bucket's time is ordinary screen time that ALSO spends its own
    /// allowance. `#[serde(default)]` so pre-buckets snapshots restore empty.
    #[serde(default)]
    bucket_today_secs: BTreeMap<String, u64>,
    /// Week-keyed twin of `bucket_today_secs` — a bucket may cap the day, the
    /// week, or both (`AppBucket.weekly_minutes`), so it needs its own meter
    /// that survives the day roll and only clears on the week roll.
    /// `#[serde(default)]` so pre-weekly-bucket snapshots restore empty.
    #[serde(default)]
    bucket_week_secs: BTreeMap<String, u64>,
    /// Whether the caller vouches for the wall clock at this moment (N1):
    /// on Linux, the kernel reports it NTP-synchronised. It gates ONLY the
    /// drop-back of a far-future day/week key in [`roll`](Self::roll); see
    /// there. Deliberately not persisted (`#[serde(skip)]`): it is a fact
    /// about the clock now, not about the ledger, so every restored or fresh
    /// ledger starts untrusted — pure high-water — until the caller says
    /// otherwise via [`set_clock_trusted`](Self::set_clock_trusted).
    #[serde(skip)]
    clock_trusted: bool,
    /// The day-keyed counters each day roll set aside since the clock was
    /// last confirmed (R2-1), oldest first. A forward firmware trip rolls the
    /// day and zeroes the counters; when the kernel later vouches for the
    /// clock and the key drops back, the counters set aside for the real day
    /// (and every excursion day after it) are added back, so the trip is
    /// worth nothing. See [`roll`](Self::roll). Persisted with the ledger:
    /// the return from a trip is normally a reboot. `#[serde(default)]` so
    /// older snapshots restore with an empty chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pre_roll_days: Vec<DayCounters>,
    /// The week-keyed twin of `pre_roll_days`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pre_roll_weeks: Vec<WeekCounters>,
}

/// The most pre-roll entries a chain keeps. Past it, the two NEWEST are
/// merged under the newer key (R3-1). The chain's first (oldest) key is never
/// touched, so [`chain_restores`] still sees the chain reach back to the real
/// day; and the merged entry carries the later key, so a restore takes it
/// whenever either half would have been taken — it can only ever over-charge
/// (an earlier fake day counted into today), never hand time back.
const PRE_ROLL_CAP: usize = 16;

/// The day-keyed counters set aside by one day roll (R2-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DayCounters {
    key: String,
    #[serde(default)]
    used_today_secs: u64,
    #[serde(default)]
    learning_today_secs: u64,
    #[serde(default)]
    unrecognised_today_secs: u64,
    #[serde(default)]
    out_of_hours_today_secs: u64,
    #[serde(default)]
    minutes_today: MinuteSet,
    #[serde(default)]
    bucket_today_secs: BTreeMap<String, u64>,
}

/// The week-keyed counters set aside by one week roll (R2-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WeekCounters {
    key: String,
    #[serde(default)]
    used_week_secs: u64,
    #[serde(default)]
    out_of_hours_week_secs: u64,
    #[serde(default)]
    out_of_hours_nights_week: u32,
    #[serde(default)]
    bucket_week_secs: BTreeMap<String, u64>,
}

fn add_buckets(into: &mut BTreeMap<String, u64>, from: &BTreeMap<String, u64>) {
    for (k, v) in from {
        let e = into.entry(k.clone()).or_insert(0);
        *e = e.saturating_add(*v);
    }
}

impl DayCounters {
    /// Fold `other`'s counters into these (the key is the caller's business).
    fn absorb(&mut self, other: &DayCounters) {
        self.used_today_secs = self.used_today_secs.saturating_add(other.used_today_secs);
        self.learning_today_secs = self
            .learning_today_secs
            .saturating_add(other.learning_today_secs);
        self.unrecognised_today_secs = self
            .unrecognised_today_secs
            .saturating_add(other.unrecognised_today_secs);
        self.out_of_hours_today_secs = self
            .out_of_hours_today_secs
            .saturating_add(other.out_of_hours_today_secs);
        self.minutes_today = self.minutes_today.union(&other.minutes_today);
        add_buckets(&mut self.bucket_today_secs, &other.bucket_today_secs);
    }
}

impl WeekCounters {
    fn absorb(&mut self, other: &WeekCounters) {
        self.used_week_secs = self.used_week_secs.saturating_add(other.used_week_secs);
        self.out_of_hours_week_secs = self
            .out_of_hours_week_secs
            .saturating_add(other.out_of_hours_week_secs);
        self.out_of_hours_nights_week = self
            .out_of_hours_nights_week
            .saturating_add(other.out_of_hours_nights_week);
        add_buckets(&mut self.bucket_week_secs, &other.bucket_week_secs);
    }
}

/// A pre-roll chain entry: anything with a date key that can absorb another.
trait PreRoll: Clone {
    fn key(&self) -> &str;
    fn set_key(&mut self, key: String);
    fn merge(&mut self, other: &Self);
}

impl PreRoll for DayCounters {
    fn key(&self) -> &str {
        &self.key
    }
    fn set_key(&mut self, key: String) {
        self.key = key;
    }
    fn merge(&mut self, other: &Self) {
        self.absorb(other);
    }
}

impl PreRoll for WeekCounters {
    fn key(&self) -> &str {
        &self.key
    }
    fn set_key(&mut self, key: String) {
        self.key = key;
    }
    fn merge(&mut self, other: &Self) {
        self.absorb(other);
    }
}

/// Append a pre-roll entry, keeping the chain within [`PRE_ROLL_CAP`] by
/// merging the two newest under the later key. Merging the oldest instead
/// would move the chain's first key forward past the real day and disable
/// the restore on return (R3-1).
fn push_pre_roll<T: PreRoll>(chain: &mut Vec<T>, entry: T) {
    chain.push(entry);
    while chain.len() > PRE_ROLL_CAP {
        let newest = chain.pop().expect("chain is over the cap");
        let prev = chain.last_mut().expect("the cap is at least two");
        let later = std::cmp::max(prev.key(), newest.key()).to_string();
        prev.merge(&newest);
        prev.set_key(later);
    }
}

/// Whether a trusted clock at computed key `current` should take back the
/// counters `chain` set aside: the stored key is ahead of `current`, and the
/// chain reaches back to (or before) it — so the lead is the product of rolls
/// that began on or before the real day, and every entry keyed `current` or
/// later was really spent today.
fn chain_restores<T: PreRoll>(trusted: bool, chain: &[T], stored: &str, current: &str) -> bool {
    trusted && stored > current && chain.first().is_some_and(|e| e.key() <= current)
}

/// Remove and fold together every entry keyed `current` or later.
fn take_restorable<T: PreRoll>(chain: &mut Vec<T>, current: &str) -> Option<T> {
    let mut out: Option<T> = None;
    for e in chain.drain(..).filter(|e| e.key() >= current) {
        match out.as_mut() {
            Some(o) => o.merge(&e),
            None => {
                let mut e = e;
                e.set_key(current.to_string());
                out = Some(e);
            }
        }
    }
    out
}

fn local(now_unix: i64, tz: Tz) -> DateTime<Tz> {
    DateTime::from_timestamp(now_unix, 0)
        .expect("valid timestamp")
        .with_timezone(&tz)
}

fn day_key_of(dt: &DateTime<Tz>) -> String {
    dt.format("%Y-%m-%d").to_string()
}

/// The start-of-week date key (the most recent `week_start` day on/before `dt`).
fn week_key_of(dt: &DateTime<Tz>, week_start: WeekStart) -> String {
    let dow = dt.weekday().num_days_from_sunday(); // Sun=0..Sat=6
    let start_dow = match week_start {
        WeekStart::Sun => 0,
        WeekStart::Mon => 1,
    };
    let back = (dow + 7 - start_dow) % 7;
    // Subtract CALENDAR days (DST-safe), not a fixed 24h*back Duration which can
    // cross a DST boundary and mis-key the week near midnight.
    let start = dt.date_naive() - chrono::Days::new(back as u64);
    start.format("%Y-%m-%d").to_string()
}

/// How many calendar days the `YYYY-MM-DD` key `stored` lies after `current`
/// (negative when behind); `None` if either will not parse.
fn days_ahead(stored: &str, current: &str) -> Option<i64> {
    let parse = |k: &str| chrono::NaiveDate::parse_from_str(k, "%Y-%m-%d").ok();
    Some((parse(stored)? - parse(current)?).num_days())
}

impl UsageLedger {
    /// A fresh ledger for `tz` at `now`.
    pub fn new(tz: &str, week_start: WeekStart, now_unix: i64) -> Self {
        let parsed: Tz = tz.parse().unwrap_or(chrono_tz::UTC);
        let dt = local(now_unix, parsed);
        UsageLedger {
            tz: tz.to_string(),
            week_start,
            day_key: day_key_of(&dt),
            week_key: week_key_of(&dt, week_start),
            used_today_secs: 0,
            used_week_secs: 0,
            learning_today_secs: 0,
            unrecognised_today_secs: 0,
            out_of_hours_today_secs: 0,
            out_of_hours_week_secs: 0,
            out_of_hours_nights_week: 0,
            minutes_today: MinuteSet::default(),
            bucket_today_secs: BTreeMap::new(),
            bucket_week_secs: BTreeMap::new(),
            clock_trusted: false,
            pre_roll_days: Vec::new(),
            pre_roll_weeks: Vec::new(),
        }
    }

    /// Tell the ledger whether the wall clock can be trusted right now —
    /// `true` only when the platform can vouch for it (Linux: the kernel
    /// reports it NTP-synchronised). Untrusted is the default and the
    /// fail-safe: the day/week keys are then a pure high-water mark. Call it
    /// before each tick's credits; it is not persisted.
    pub fn set_clock_trusted(&mut self, trusted: bool) {
        self.clock_trusted = trusted;
    }

    /// Whether a stored key `ahead` days in front of the computed one should
    /// be dropped back to it. An unparseable key always is (corruption
    /// recovery; the ledger file is not the ward's to write). A far-future key
    /// is only while the clock is trusted — see [`roll`](Self::roll).
    fn drop_back(&self, ahead: Option<i64>, bound: i64) -> bool {
        ahead.is_none_or(|d| self.clock_trusted && d > bound)
    }

    fn tz(&self) -> Tz {
        self.tz.parse().unwrap_or(chrono_tz::UTC)
    }

    /// The tz name the ledger's day and week keys are currently anchored on.
    pub fn tz_name(&self) -> &str {
        &self.tz
    }

    /// Whether the ledger's day has ADVANCED past the one its counters belong
    /// to — the reader's half of [`roll`](Self::roll)'s high-water rule. Every
    /// day-keyed reader answers zero on this and only this: a clock that has
    /// gone backwards leaves `false`, so today's spent seconds stay spent
    /// instead of reading as "a different day, nothing used".
    fn day_has_rolled(&self, now_unix: i64) -> bool {
        day_key_of(&local(now_unix, self.tz())) > self.day_key
    }

    /// The week-keyed twin of [`day_has_rolled`](Self::day_has_rolled).
    fn week_has_rolled(&self, now_unix: i64) -> bool {
        week_key_of(&local(now_unix, self.tz()), self.week_start) > self.week_key
    }

    /// Roll the day/week keys at `now`, resetting the matching counters when the
    /// local calendar day/week **advances**.
    ///
    /// The stored keys are a HIGH-WATER MARK: the roll takes
    /// `max(latest seen, computed)`, so a wall clock that moves backwards can
    /// never roll the day back and hand a spent quota out again. The polkit
    /// `timedate1` deny-set only covers the in-OS path; the named top adversary
    /// is a child with physical access, and setting the RTC in UEFI setup (or
    /// booting single-user) goes round it entirely. A forward step behaves
    /// exactly as it always did — including an NTP correction forward — so the
    /// only thing given up is the ability to REVERSE a roll that already
    /// happened. That is the residual: an RTC that reads a future date rolls
    /// the day once, and the correction back does not restore it.
    ///
    /// The high-water mark is bounded, though (M4) — but only while the
    /// clock is TRUSTED ([`set_clock_trusted`](Self::set_clock_trusted)). One
    /// tick with the clock years ahead — a bad RTC — used to set the key into
    /// the future, and once the clock was right again `dk > day_key` stayed
    /// false for years: the counters never reset and the ward was locked every
    /// day once the first day's cap was spent. So, with a trusted clock, a
    /// stored key more than one day (or, for the week, one week) AHEAD of the
    /// computed one is pulled back to it, WITHOUT zeroing anything: the
    /// counters carry on and the next real midnight resets them as normal.
    ///
    /// Why only when trusted (N1): an untrusted drop-back is itself a lever.
    /// The firmware RTC is read at boot before NTP runs, so a ward who sets
    /// it two days back and keeps the Wi-Fi off would get the key dropped
    /// back to T-2, and the NTP step forward on reconnecting would then roll
    /// "a new day" and refill the quota — repeatably, one refill per trip to
    /// the firmware. Untrusted, the keys stay a pure high-water mark, so a
    /// step back and return is a no-op, and a future excursion stays frozen
    /// (locked, the fail-safe direction) until the kernel reports the clock
    /// synchronised, when it recovers here. An unparseable key is re-anchored
    /// regardless.
    ///
    /// A forward excursion still rolls the day (a high-water mark cannot tell
    /// it from a real midnight), so each roll sets the counters it zeroes
    /// aside in a pre-roll chain (R2-1), persisted with the ledger. A roll on
    /// a trusted clock is a real midnight and clears the chain, and so does
    /// any trusted tick that finds the stored key current: from then on the
    /// counters are the real day's. When a trusted clock finds the stored key
    /// AHEAD and the chain reaching back to the computed day, the lead came
    /// from rolls a wrong clock made, so every entry keyed today or later was
    /// really spent today: it is added back and the key re-anchored, whatever
    /// the size of the lead. A forward trip to the firmware is therefore
    /// worth nothing once the clock is synchronised again, and neither is a
    /// chain of fake midnights. Without a chain the bounded drop-back above
    /// applies, as before. Android pins the clock under Device Owner and
    /// never sets the flag, so it keeps the pure high-water mark.
    ///
    /// Day keys are `YYYY-MM-DD` and week keys are the start-of-week date in
    /// the same shape, so lexicographic order IS chronological order.
    fn roll(&mut self, now_unix: i64) {
        let dt = local(now_unix, self.tz());
        let dk = day_key_of(&dt);
        let wk = week_key_of(&dt, self.week_start);
        let mut restored_day: Option<DayCounters> = None;
        if dk > self.day_key {
            let old = DayCounters {
                key: std::mem::replace(&mut self.day_key, dk.clone()),
                used_today_secs: std::mem::take(&mut self.used_today_secs),
                learning_today_secs: std::mem::take(&mut self.learning_today_secs),
                unrecognised_today_secs: std::mem::take(&mut self.unrecognised_today_secs),
                out_of_hours_today_secs: std::mem::take(&mut self.out_of_hours_today_secs),
                minutes_today: std::mem::take(&mut self.minutes_today),
                bucket_today_secs: std::mem::take(&mut self.bucket_today_secs),
            };
            if !self.clock_trusted {
                push_pre_roll(&mut self.pre_roll_days, old);
            }
        } else if chain_restores(self.clock_trusted, &self.pre_roll_days, &self.day_key, &dk) {
            // Back from a forward excursion on a trusted clock: what the
            // excursion's rolls set aside was spent today. Add it back.
            restored_day = take_restorable(&mut self.pre_roll_days, &dk);
            self.day_key = dk.clone();
            if let Some(r) = &restored_day {
                let mut cur = DayCounters {
                    key: dk.clone(),
                    used_today_secs: self.used_today_secs,
                    learning_today_secs: self.learning_today_secs,
                    unrecognised_today_secs: self.unrecognised_today_secs,
                    out_of_hours_today_secs: self.out_of_hours_today_secs,
                    minutes_today: std::mem::take(&mut self.minutes_today),
                    bucket_today_secs: std::mem::take(&mut self.bucket_today_secs),
                };
                cur.absorb(r);
                self.used_today_secs = cur.used_today_secs;
                self.learning_today_secs = cur.learning_today_secs;
                self.unrecognised_today_secs = cur.unrecognised_today_secs;
                self.out_of_hours_today_secs = cur.out_of_hours_today_secs;
                self.minutes_today = cur.minutes_today;
                self.bucket_today_secs = cur.bucket_today_secs;
            }
        } else if self.drop_back(days_ahead(&self.day_key, &dk), 1) {
            // Back from a future excursion on a trusted clock (or an
            // unparseable key): re-anchor, keep the counters.
            self.day_key = dk.clone();
        }
        if self.clock_trusted && self.day_key == dk {
            // The clock vouches for the day the counters are on: the chain
            // is closed.
            self.pre_roll_days.clear();
        }
        if wk > self.week_key {
            let old = WeekCounters {
                key: std::mem::replace(&mut self.week_key, wk.clone()),
                used_week_secs: std::mem::take(&mut self.used_week_secs),
                out_of_hours_week_secs: std::mem::take(&mut self.out_of_hours_week_secs),
                out_of_hours_nights_week: std::mem::take(&mut self.out_of_hours_nights_week),
                bucket_week_secs: std::mem::take(&mut self.bucket_week_secs),
            };
            if !self.clock_trusted {
                push_pre_roll(&mut self.pre_roll_weeks, old);
            }
        } else if chain_restores(
            self.clock_trusted,
            &self.pre_roll_weeks,
            &self.week_key,
            &wk,
        ) {
            if let Some(r) = take_restorable(&mut self.pre_roll_weeks, &wk) {
                self.used_week_secs = self.used_week_secs.saturating_add(r.used_week_secs);
                self.out_of_hours_week_secs = self
                    .out_of_hours_week_secs
                    .saturating_add(r.out_of_hours_week_secs);
                self.out_of_hours_nights_week = self
                    .out_of_hours_nights_week
                    .saturating_add(r.out_of_hours_nights_week);
                add_buckets(&mut self.bucket_week_secs, &r.bucket_week_secs);
            }
            self.week_key = wk.clone();
        } else if self.drop_back(days_ahead(&self.week_key, &wk), 7) {
            self.week_key = wk.clone();
        }
        if self.clock_trusted && self.week_key == wk {
            self.pre_roll_weeks.clear();
        }
        // A night the excursion counted a second time (out-of-hours use on
        // both the real day and a fake one) is still one night.
        if let Some(r) = restored_day {
            if r.out_of_hours_today_secs > 0
                && self.out_of_hours_today_secs > r.out_of_hours_today_secs
            {
                self.out_of_hours_nights_week = self.out_of_hours_nights_week.saturating_sub(1);
            }
        }
    }

    /// Credit `elapsed_secs` of `activity` at `now`. Only Active time counts;
    /// day/week boundaries reset first (so a restart cannot refill the quota).
    pub fn credit(&mut self, now_unix: i64, activity: Activity, elapsed_secs: u64) {
        self.credit_bucket(now_unix, activity, Bucket::Screen, elapsed_secs);
    }

    /// Credit into a specific bucket. `Screen` is exactly [`credit`]'s
    /// semantics; `Learning` feeds the day-keyed learning counter only —
    /// the budget's day/week totals are untouched.
    pub fn credit_bucket(
        &mut self,
        now_unix: i64,
        activity: Activity,
        bucket: Bucket,
        elapsed_secs: u64,
    ) {
        self.roll(now_unix);
        if !activity.counts() {
            return;
        }
        match bucket {
            Bucket::Screen => {
                self.used_today_secs = self.used_today_secs.saturating_add(elapsed_secs);
                self.used_week_secs = self.used_week_secs.saturating_add(elapsed_secs);
            }
            Bucket::Learning => {
                self.learning_today_secs = self.learning_today_secs.saturating_add(elapsed_secs);
            }
        }
        // Either bucket is time at the glass: journal the minutes.
        self.minutes_today
            .mark_span(self.tz(), now_unix, elapsed_secs);
    }

    /// Credit `elapsed_secs` to the unrecognised-time counter (spec §2.3):
    /// foreground screen time whose resolved identity was ABSENT from the
    /// device's own inventory — software Charter does not know about at all,
    /// never merely an installed app that happens to be in no group (that
    /// case must call [`credit_bucket`](Self::credit_bucket) alone, crediting
    /// nothing here). Called ALONGSIDE the ordinary `Screen` credit, exactly
    /// like [`credit_app_bucket`](Self::credit_app_bucket) — this is
    /// additional information about screen time already credited, not a
    /// second pool of time. Only Active time counts, same as every other
    /// meter here.
    pub fn credit_unrecognised(&mut self, now_unix: i64, activity: Activity, elapsed_secs: u64) {
        self.roll(now_unix);
        if !activity.counts() {
            return;
        }
        self.unrecognised_today_secs = self.unrecognised_today_secs.saturating_add(elapsed_secs);
    }

    /// Unrecognised-time seconds today (0 once the local calendar day
    /// changed) — the twin of [`learning_today_secs`](Self::learning_today_secs).
    pub fn unrecognised_today_secs(&self, now_unix: i64) -> u64 {
        if self.day_has_rolled(now_unix) {
            0
        } else {
            self.unrecognised_today_secs
        }
    }

    /// Credit out-of-hours seconds (spec 2026-08-03). Never touches
    /// `used_today_secs` or `used_week_secs` — this is a fact ABOUT the day,
    /// not a pool of time. Takes no timestamp: unlike the `credit_*` family
    /// this never rolls the day/week keys itself. It is called from the
    /// warden tick immediately after the ordinary per-tick usage credit for
    /// the same `now`, which has already rolled the keys current — a second,
    /// independent roll here would risk drifting from that call's idea of
    /// "today".
    pub fn mark_out_of_hours(&mut self, secs: u64) {
        if secs == 0 {
            return;
        }
        // The 0 → non-zero crossing IS the night. Counted before the add, so
        // a second waking the same night does not count twice.
        if self.out_of_hours_today_secs == 0 {
            self.out_of_hours_nights_week = self.out_of_hours_nights_week.saturating_add(1);
        }
        self.out_of_hours_today_secs = self.out_of_hours_today_secs.saturating_add(secs);
        self.out_of_hours_week_secs = self.out_of_hours_week_secs.saturating_add(secs);
    }

    /// Out-of-hours seconds accrued so far in the current day key.
    pub fn out_of_hours_today_secs(&self) -> u64 {
        self.out_of_hours_today_secs
    }

    /// Out-of-hours seconds accrued so far in the current week key — the
    /// week-keyed twin of [`out_of_hours_today_secs`](Self::out_of_hours_today_secs).
    pub fn out_of_hours_week_secs(&self) -> u64 {
        self.out_of_hours_week_secs
    }

    /// Count of distinct days this week that saw any out-of-hours use — the
    /// "3 nights" half of the guardian's line.
    pub fn out_of_hours_nights_week(&self) -> u32 {
        self.out_of_hours_nights_week
    }

    /// Credit an app bucket's daily AND weekly meters. Called ALONGSIDE the
    /// ordinary `Screen` credit, not instead of it: an hour of Minecraft is an
    /// hour of screen time AND an hour of Play. (Learning is the opposite
    /// special case — free of the day budget entirely.) Both meters move
    /// together here; which one(s) actually cap the bucket is a policy
    /// question answered elsewhere (`AppBucket.daily_minutes` /
    /// `weekly_minutes`).
    pub fn credit_app_bucket(&mut self, now_unix: i64, bucket_id: &str, elapsed_secs: u64) {
        self.roll(now_unix);
        let day = self
            .bucket_today_secs
            .entry(bucket_id.to_string())
            .or_insert(0);
        *day = day.saturating_add(elapsed_secs);
        let week = self
            .bucket_week_secs
            .entry(bucket_id.to_string())
            .or_insert(0);
        *week = week.saturating_add(elapsed_secs);
    }

    /// Seconds spent in one app bucket today (0 once the calendar day changed,
    /// so a stale ledger can never keep an allowance spent into tomorrow).
    pub fn app_bucket_today_secs(&self, now_unix: i64, bucket_id: &str) -> u64 {
        if self.day_has_rolled(now_unix) {
            0
        } else {
            self.bucket_today_secs.get(bucket_id).copied().unwrap_or(0)
        }
    }

    /// Seconds spent in one app bucket this week (0 once the local week key
    /// changed, so a stale ledger can never keep an allowance spent into next
    /// week). The week-keyed twin of [`app_bucket_today_secs`].
    pub fn app_bucket_week_secs(&self, now_unix: i64, bucket_id: &str) -> u64 {
        if self.week_has_rolled(now_unix) {
            0
        } else {
            self.bucket_week_secs.get(bucket_id).copied().unwrap_or(0)
        }
    }

    /// Learning-bucket seconds today (0 once the local calendar day changed).
    pub fn learning_today_secs(&self, now_unix: i64) -> u64 {
        if self.day_has_rolled(now_unix) {
            0
        } else {
            self.learning_today_secs
        }
    }

    /// The CURRENT local day key (`YYYY-MM-DD`) at `now` in the ledger's tz —
    /// what a USAGE_SYNC view's `dayKey` must match to count (B3).
    ///
    /// Monotonic, like the counters it keys: the later of the local calendar
    /// day and the latest day the ledger has already seen. Otherwise a
    /// backwards clock step would announce yesterday's key while the ledger
    /// still held today's seconds, and a guardian's view of today would stop
    /// matching the device that produced it.
    pub fn current_day_key(&self, now_unix: i64) -> String {
        let dk = day_key_of(&local(now_unix, self.tz()));
        // Mirrors `roll`: on a trusted clock a key more than a day in the
        // future is re-anchored.
        if dk > self.day_key
            || chain_restores(self.clock_trusted, &self.pre_roll_days, &self.day_key, &dk)
            || self.drop_back(days_ahead(&self.day_key, &dk), 1)
        {
            dk
        } else {
            self.day_key.clone()
        }
    }

    /// The CURRENT local week key at `now` in the ledger's tz — monotonic for
    /// the same reason as [`current_day_key`](Self::current_day_key).
    pub fn current_week_key(&self, now_unix: i64) -> String {
        let wk = week_key_of(&local(now_unix, self.tz()), self.week_start);
        if wk > self.week_key
            || chain_restores(
                self.clock_trusted,
                &self.pre_roll_weeks,
                &self.week_key,
                &wk,
            )
            || self.drop_back(days_ahead(&self.week_key, &wk), 7)
        {
            wk
        } else {
            self.week_key.clone()
        }
    }

    /// Today's active-minutes journal (empty once the local day has changed,
    /// mirroring [`used_today`]).
    pub fn minutes_today(&self, now_unix: i64) -> MinuteSet {
        if self.day_has_rolled(now_unix) {
            MinuteSet::default()
        } else {
            self.minutes_today.clone()
        }
    }

    /// Seconds used today (0 once the local calendar day has changed).
    pub fn used_today(&self, now_unix: i64) -> u64 {
        if self.day_has_rolled(now_unix) {
            0
        } else {
            self.used_today_secs
        }
    }

    /// Seconds used this week (0 once the local week key has changed).
    pub fn used_week(&self, now_unix: i64) -> u64 {
        if self.week_has_rolled(now_unix) {
            0
        } else {
            self.used_week_secs
        }
    }

    /// Serialize for durable persistence.
    pub fn snapshot(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Restore from a snapshot (`None` if malformed).
    pub fn from_snapshot(s: &str) -> Option<UsageLedger> {
        serde_json::from_str(s).ok()
    }

    /// Re-key the ledger to a new tz / week_start (e.g. the guardian edited the
    /// budget clause) WITHOUT refilling spent quota. A no-op when unchanged.
    pub fn reconcile(&mut self, tz: &str, week_start: WeekStart, now_unix: i64) {
        if self.tz == tz && self.week_start == week_start {
            return;
        }
        let parsed: Tz = tz.parse().unwrap_or(chrono_tz::UTC);
        let dt = local(now_unix, parsed);
        self.tz = tz.to_string();
        self.week_start = week_start;
        // Re-anchor the keys to "now" in the new tz; the used_* totals are
        // PRESERVED so a clause edit can never refill the quota.
        self.day_key = day_key_of(&dt);
        self.week_key = week_key_of(&dt, week_start);
        // The chains are keyed in the old tz; they cannot be compared with
        // the new keys, and a guardian's edit is not a ward's lever.
        self.pre_roll_days.clear();
        self.pre_roll_weeks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: &str = "Europe/London";
    // 2026-06-29 12:00 BST (a Monday).
    const NOON: i64 = 1_782_734_400;

    #[test]
    fn learning_credit_does_not_drain_screen() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_bucket(NOON, Activity::Active, Bucket::Learning, 900);
        assert_eq!(u.used_today(NOON), 0);
        assert_eq!(u.used_week(NOON), 0);
        assert_eq!(u.learning_today_secs(NOON), 900);
        // Screen credit is untouched by the learning counter and vice versa.
        u.credit_bucket(NOON, Activity::Active, Bucket::Screen, 300);
        assert_eq!(u.used_today(NOON), 300);
        assert_eq!(u.learning_today_secs(NOON), 900);
        // Non-active learning time doesn't count either.
        u.credit_bucket(NOON, Activity::Idle, Bucket::Learning, 600);
        assert_eq!(u.learning_today_secs(NOON), 900);
    }

    #[test]
    fn learning_rolls_over_at_local_midnight() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_bucket(NOON, Activity::Active, Bucket::Learning, 3600);
        assert_eq!(u.learning_today_secs(NOON), 3600);
        let next_day = NOON + 24 * 3600;
        assert_eq!(u.learning_today_secs(next_day), 0);
        u.credit_bucket(next_day, Activity::Active, Bucket::Learning, 60);
        assert_eq!(u.learning_today_secs(next_day), 60);
    }

    #[test]
    fn unrecognised_credit_does_not_drain_screen_or_learning() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_unrecognised(NOON, Activity::Active, 900);
        assert_eq!(u.unrecognised_today_secs(NOON), 900);
        assert_eq!(u.used_today(NOON), 0);
        assert_eq!(u.learning_today_secs(NOON), 0);
        // Ordinary screen credit accrues alongside it (it is ADDITIONAL
        // information about time already credited, not a separate pool).
        u.credit_bucket(NOON, Activity::Active, Bucket::Screen, 900);
        assert_eq!(u.used_today(NOON), 900);
        assert_eq!(u.unrecognised_today_secs(NOON), 900);
        // Idle time never counts, same as every other meter.
        u.credit_unrecognised(NOON, Activity::Idle, 600);
        assert_eq!(u.unrecognised_today_secs(NOON), 900);
    }

    #[test]
    fn unrecognised_rolls_over_at_local_midnight() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_unrecognised(NOON, Activity::Active, 3600);
        assert_eq!(u.unrecognised_today_secs(NOON), 3600);
        let next_day = NOON + 24 * 3600;
        assert_eq!(u.unrecognised_today_secs(next_day), 0);
        u.credit_unrecognised(next_day, Activity::Active, 60);
        assert_eq!(u.unrecognised_today_secs(next_day), 60);
    }

    /// Out-of-hours time is a FACT ABOUT the day, never a pool of time. It must
    /// never reach the budget: charging her for a bad night's sleep is the
    /// counting and the enforcing disagreeing.
    #[test]
    fn out_of_hours_time_never_touches_the_budget() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        let before = u.used_today(NOON);
        u.mark_out_of_hours(600);
        assert_eq!(u.out_of_hours_today_secs(), 600);
        assert_eq!(u.used_today(NOON), before, "the budget must not move");
    }

    #[test]
    fn out_of_hours_accumulates_and_resets_with_the_day() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_today_secs(), 600);
        // A day later, the counter is a fresh day's counter. `mark_out_of_hours`
        // takes no timestamp and never rolls on its own (see its doc comment);
        // in production the ordinary per-tick `credit` call for the same `now`
        // rolls the keys first, so a zero-effect `credit` call here stands in
        // for that.
        let next_day = NOON + 86_400 * 2;
        u.credit(next_day, Activity::Idle, 0);
        assert_eq!(u.out_of_hours_today_secs(), 0);
    }

    /// Three wakings in one night are ONE night. The line says "3 nights this
    /// week", not "3 times".
    #[test]
    fn several_wakings_in_one_night_count_as_one_night() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_nights_week(), 1);
        assert_eq!(u.out_of_hours_week_secs(), 900);
    }

    #[test]
    fn a_zero_credit_never_invents_a_night() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.mark_out_of_hours(0);
        assert_eq!(u.out_of_hours_nights_week(), 0);
        assert_eq!(u.out_of_hours_today_secs(), 0);
    }

    /// The day counter rolls nightly; the week total and the night count do
    /// not — they roll with the week, beside `used_week_secs`.
    #[test]
    fn a_second_night_adds_to_the_week_after_the_day_rolls() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.mark_out_of_hours(600);
        let next_day = NOON + 86_400;
        u.credit(next_day, Activity::Idle, 0);
        assert_eq!(u.out_of_hours_today_secs(), 0, "a fresh day");
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_nights_week(), 2);
        assert_eq!(u.out_of_hours_week_secs(), 900);
    }

    /// Snapshots written before this counter existed must restore cleanly.
    /// Field shape confirmed by dumping a real `UsageLedger::new(...).snapshot()`
    /// (no `rename_all` on the struct, so keys are the literal snake_case Rust
    /// field names, and `week_key`/`day_key` are both `YYYY-MM-DD` date
    /// strings — the plan's illustrative `"weekKey":"2026-W32"` camelCase/ISO
    /// shape does not match and was not used here).
    #[test]
    fn a_pre_existing_snapshot_restores_with_a_zero_counter() {
        let old = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        let mut v: serde_json::Value = serde_json::from_str(&old.snapshot()).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("out_of_hours_today_secs")
            .expect("field present in new snapshots");
        obj.remove("out_of_hours_week_secs")
            .expect("field present in new snapshots");
        obj.remove("out_of_hours_nights_week")
            .expect("field present in new snapshots");
        let u = UsageLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(u.out_of_hours_today_secs(), 0);
        assert_eq!(u.out_of_hours_week_secs(), 0);
        assert_eq!(u.out_of_hours_nights_week(), 0);
    }

    #[test]
    fn snapshot_without_unrecognised_field_restores() {
        // A pre-§2.3 snapshot (no unrecognised_today_secs key) must restore
        // cleanly with a zero counter — upgrades keep spent quota.
        let mut old = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        old.credit(NOON, Activity::Active, 1200);
        let mut v: serde_json::Value = serde_json::from_str(&old.snapshot()).unwrap();
        v.as_object_mut()
            .unwrap()
            .remove("unrecognised_today_secs")
            .expect("field present in new snapshots");
        let restored = UsageLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(restored.used_today(NOON), 1200);
        assert_eq!(restored.unrecognised_today_secs(NOON), 0);
    }

    #[test]
    fn snapshot_without_learning_field_restores() {
        // A pre-learning snapshot (no learning_today_secs key) must restore
        // cleanly with a zero learning counter — upgrades keep spent quota.
        let mut old = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        old.credit(NOON, Activity::Active, 1200);
        let mut v: serde_json::Value = serde_json::from_str(&old.snapshot()).unwrap();
        v.as_object_mut()
            .unwrap()
            .remove("learning_today_secs")
            .expect("field present in new snapshots");
        let restored = UsageLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(restored.used_today(NOON), 1200);
        assert_eq!(restored.learning_today_secs(NOON), 0);
    }

    #[test]
    fn excluded_states_credit_zero() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        for a in [
            Activity::Idle,
            Activity::Locked,
            Activity::Suspended,
            Activity::FrozenByCharter,
        ] {
            u.credit(NOON, a, 600);
        }
        assert_eq!(u.used_today(NOON), 0);
    }

    #[test]
    fn active_time_accrues() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 600);
        u.credit(NOON + 600, Activity::Active, 600);
        assert_eq!(u.used_today(NOON + 600), 1200);
        assert_eq!(u.used_week(NOON + 600), 1200);
    }

    #[test]
    fn per_day_reset_at_tz_midnight() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 3600);
        assert_eq!(u.used_today(NOON), 3600);
        // 24h later -> next day -> reset.
        let next_day = NOON + 24 * 3600;
        assert_eq!(u.used_today(next_day), 0);
        u.credit(next_day, Activity::Active, 60);
        assert_eq!(u.used_today(next_day), 60);
        // Week total still accumulates within the same week.
        assert_eq!(u.used_week(next_day), 3660);
    }

    /// 03-G6: a firmware clock rollback must not reset the day's quota. The
    /// polkit `timedate1` deny-set only covers the in-OS path — the RTC in
    /// UEFI setup goes round it — so the ledger's day key is a high-water
    /// mark.
    #[test]
    fn a_backwards_clock_step_never_rolls_the_day_back() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 3600);
        assert_eq!(u.used_today(NOON), 3600);

        // The clock steps back a day. The reader must still answer 3600…
        let yesterday = NOON - 24 * 3600;
        assert_eq!(u.used_today(yesterday), 3600, "quota is not handed back");
        assert_eq!(u.current_day_key(yesterday), u.current_day_key(NOON));
        // …and a credit at the rolled-back instant ADDS rather than resetting.
        u.credit(yesterday, Activity::Active, 600);
        assert_eq!(u.used_today(yesterday), 4200);
        assert_eq!(u.used_today(NOON), 4200);

        // A whole week back is the same answer, and so is the week meter.
        let last_week = NOON - 7 * 24 * 3600;
        assert_eq!(u.used_today(last_week), 4200);
        assert_eq!(u.used_week(last_week), 4200);

        // Then forward past the real boundary: the day rolls ONCE, as always.
        let tomorrow = NOON + 24 * 3600;
        assert_eq!(u.used_today(tomorrow), 0);
        u.credit(tomorrow, Activity::Active, 60);
        assert_eq!(u.used_today(tomorrow), 60);
        assert_eq!(u.used_week(tomorrow), 4260, "same week keeps accumulating");
        // …and the new day is itself now the floor.
        assert_eq!(u.used_today(NOON), 60);
    }

    /// M4: one tick with the clock far in the future, then the correction.
    /// The future tick rolls the day once (the residual), but the stored key
    /// must come back, or no real midnight would ever reset the day again.
    #[test]
    fn a_future_clock_excursion_does_not_freeze_the_day_key() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 600);
        let far = NOON + 5 * 365 * 24 * 3600;
        u.credit(far, Activity::Active, 60);
        // Corrected, but not yet synchronised: the key stays frozen ahead
        // (the fail-safe direction)…
        u.credit(NOON + 30, Activity::Active, 0);
        assert_eq!(u.current_day_key(NOON + 30), u.current_day_key(far));
        assert_eq!(u.used_today(NOON + 30), 60);
        // …and once the kernel vouches for the clock, the counters are kept,
        // not zeroed, and what the excursion's roll set aside comes back
        // (R2-1)…
        u.set_clock_trusted(true);
        u.credit(NOON + 60, Activity::Active, 60);
        assert_eq!(u.used_today(NOON + 60), 720, "no refill on the way back");
        assert_eq!(u.used_week(NOON + 60), 720);
        assert_eq!(u.current_day_key(NOON + 60), u.current_day_key(NOON));
        // …and the next real midnight resets as normal.
        let tomorrow = NOON + 24 * 3600;
        u.credit(tomorrow, Activity::Active, 30);
        assert_eq!(u.used_today(tomorrow), 30, "the next day starts fresh");
        assert_eq!(u.used_week(tomorrow), 750, "same week keeps accumulating");
        let next_week = NOON + 7 * 24 * 3600;
        assert_eq!(u.used_week(next_week), 0, "and the week rolls too");
    }

    /// The bound on the high-water mark does not loosen it for a step BACK:
    /// a step back across midnight still keeps today's spend.
    #[test]
    fn a_backwards_step_still_never_refills_after_the_bound() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 3600);
        let before_midnight = NOON - 14 * 3600; // 23:00 local the day before
        u.credit(before_midnight, Activity::Active, 60);
        assert_eq!(u.used_today(before_midnight), 3660);
        u.credit(NOON + 60, Activity::Active, 60);
        assert_eq!(u.used_today(NOON + 60), 3720, "coming back does not reset");
        // A step back of several days is never zeroed either.
        let days_back = NOON - 3 * 24 * 3600;
        u.credit(days_back, Activity::Active, 60);
        assert_eq!(u.used_today(days_back), 3780);
    }

    /// N1, the firmware-RTC trip: the RTC is set two days back with the
    /// Wi-Fi off (untrusted), then NTP steps it forward on reconnecting. The
    /// return must never refill — neither while the clock is still
    /// untrusted, nor once it is synchronised again.
    #[test]
    fn back_two_days_then_return_never_refills_untrusted() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 3600);
        let back = NOON - 2 * 24 * 3600;
        u.credit(back, Activity::Active, 60);
        assert_eq!(u.used_today(back), 3660, "untrusted: pure high-water");
        assert_eq!(u.current_day_key(back), u.current_day_key(NOON));
        // Back to the right time, still untrusted.
        u.credit(NOON + 60, Activity::Active, 60);
        assert_eq!(u.used_today(NOON + 60), 3720, "no refill on return");
        assert_eq!(u.used_week(NOON + 60), 3720);
        // Synchronised at last: still no refill.
        u.set_clock_trusted(true);
        u.credit(NOON + 120, Activity::Active, 60);
        assert_eq!(u.used_today(NOON + 120), 3780, "no refill once trusted");
        // The same trip repeated is still worth nothing.
        u.set_clock_trusted(false);
        u.credit(back, Activity::Active, 0);
        u.set_clock_trusted(true);
        u.credit(NOON + 180, Activity::Active, 0);
        assert_eq!(u.used_today(NOON + 180), 3780);
    }

    /// The week twin of the trip above: ten days back and return.
    #[test]
    fn back_ten_days_then_return_never_refills_the_week_untrusted() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 3600);
        let back = NOON - 10 * 24 * 3600;
        u.credit(back, Activity::Active, 60);
        u.set_clock_trusted(true);
        u.credit(NOON + 60, Activity::Active, 60);
        assert_eq!(u.used_today(NOON + 60), 3720);
        assert_eq!(u.used_week(NOON + 60), 3720);
    }

    /// R2-1: the forward firmware trip. Two days ahead with the Wi-Fi off
    /// rolls the day and zeroes it (the high-water mark cannot tell it from
    /// midnight); once the kernel vouches for the clock again, the counters
    /// the roll set aside come back, so the trip is worth nothing.
    #[test]
    fn a_forward_trip_then_sync_gives_no_extra_allowance() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.set_clock_trusted(true);
        u.credit(NOON, Activity::Active, 3600);
        u.credit_app_bucket(NOON, "play", 600);
        u.credit_bucket(NOON, Activity::Active, Bucket::Learning, 300);
        // Firmware RTC two days ahead, booted offline: untrusted.
        u.set_clock_trusted(false);
        let ahead = NOON + 2 * 24 * 3600 + 60;
        u.credit(ahead, Activity::Active, 900);
        u.credit_app_bucket(ahead, "play", 900);
        assert_eq!(
            u.used_today(ahead),
            900,
            "the excursion looks like a fresh day"
        );
        // It survives a restart (the return is normally a reboot).
        let mut u = UsageLedger::from_snapshot(&u.snapshot()).expect("restores");
        // NTP corrects the clock and clears STA_UNSYNC.
        u.set_clock_trusted(true);
        let back = NOON + 3600;
        u.credit(back, Activity::Active, 60);
        assert_eq!(u.current_day_key(back), day_key_of(&local(NOON, u.tz())));
        assert_eq!(
            u.used_today(back),
            3600 + 900 + 60,
            "no net extra allowance"
        );
        assert_eq!(u.used_week(back), 3600 + 900 + 60);
        assert_eq!(u.app_bucket_today_secs(back, "play"), 1500);
        assert_eq!(u.learning_today_secs(back), 300);
        // Repeating the trip is still worth nothing.
        u.set_clock_trusted(false);
        u.credit(ahead + 3600, Activity::Active, 120);
        u.set_clock_trusted(true);
        u.credit(back + 60, Activity::Active, 0);
        assert_eq!(u.used_today(back + 60), 4560 + 120);
        // …and the next real midnight still resets.
        let tomorrow = NOON + 24 * 3600;
        u.credit(tomorrow, Activity::Active, 30);
        assert_eq!(u.used_today(tomorrow), 30);
    }

    /// The trip before the clock was ever confirmed today (Wi-Fi off since
    /// boot, so the real midnight roll was untrusted too), and a chain of
    /// fake midnights on the excursion: only the real day and the excursion
    /// come back, never the day before.
    #[test]
    fn a_chain_of_fake_midnights_after_an_untrusted_real_one_restores_today_only() {
        let yesterday = NOON - 24 * 3600;
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, yesterday);
        u.credit(yesterday, Activity::Active, 5000);
        // Real midnight, untrusted (booted offline).
        u.credit(NOON, Activity::Active, 3600);
        assert_eq!(u.used_today(NOON), 3600);
        // Firmware +2 days, then +3, then +4: three fake midnights.
        for d in 2..=4 {
            u.credit(NOON + d * 24 * 3600, Activity::Active, 100);
        }
        u.set_clock_trusted(true);
        u.credit(NOON + 60, Activity::Active, 0);
        assert_eq!(
            u.used_today(NOON + 60),
            3600 + 300,
            "today plus the excursion"
        );
        // NOON is a Monday, so yesterday was last week; the fake days are
        // this week, and the week meter never rolled.
        assert_eq!(u.used_week(NOON + 60), 3600 + 300, "week kept whole");
    }

    /// A trip across a week boundary refills the week too, and the week comes
    /// back the same way — even when the lead is exactly one week, which the
    /// bounded drop-back alone would leave frozen (R2-6).
    #[test]
    fn a_forward_trip_across_the_week_restores_the_week() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.set_clock_trusted(true);
        u.credit(NOON, Activity::Active, 3600);
        u.set_clock_trusted(false);
        let next_week = NOON + 7 * 24 * 3600;
        u.credit(next_week, Activity::Active, 600);
        assert_eq!(u.used_week(next_week), 600);
        u.set_clock_trusted(true);
        u.credit(NOON + 60, Activity::Active, 0);
        assert_eq!(u.used_today(NOON + 60), 4200);
        assert_eq!(u.used_week(NOON + 60), 4200);
        assert_eq!(u.current_week_key(NOON + 60), u.current_week_key(NOON));
        // And the next real week still rolls.
        assert_eq!(u.used_week(next_week), 0);
    }

    /// A genuine midnight, trusted or not, still resets — the chain never
    /// hands yesterday back.
    #[test]
    fn a_genuine_midnight_roll_still_resets() {
        for trusted in [true, false] {
            let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
            u.set_clock_trusted(trusted);
            u.credit(NOON, Activity::Active, 3600);
            let tomorrow = NOON + 24 * 3600;
            u.credit(tomorrow, Activity::Active, 60);
            // Synchronised later that day: the key is current, so nothing
            // comes back.
            u.set_clock_trusted(true);
            u.credit(tomorrow + 60, Activity::Active, 0);
            assert_eq!(u.used_today(tomorrow + 60), 60, "trusted={trusted}");
            assert!(u.pre_roll_days.is_empty(), "the chain is closed");
        }
    }

    /// Chains on a clock that is never trusted (Android) stay bounded.
    #[test]
    fn an_untrusted_chain_stays_bounded() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        for d in 1..=100 {
            u.credit(NOON + d * 24 * 3600, Activity::Active, 60);
        }
        assert!(u.pre_roll_days.len() <= PRE_ROLL_CAP);
        assert!(u.pre_roll_weeks.len() <= PRE_ROLL_CAP);
        assert_eq!(u.used_today(NOON + 100 * 24 * 3600), 60);
    }

    /// More untrusted rolls than the chain holds still restore on a trusted
    /// return (R3-1): the cap merges the newest entries, so the chain still
    /// reaches back to the real day and nothing spent is left uncharged.
    #[test]
    fn a_capped_chain_still_restores_the_real_day() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.set_clock_trusted(true);
        u.credit(NOON, Activity::Active, 3600);
        u.set_clock_trusted(false);
        let rolls: i64 = PRE_ROLL_CAP as i64 + 4;
        for d in 1..=rolls {
            u.credit(NOON + d * 24 * 3600, Activity::Active, 60);
        }
        assert!(u.pre_roll_days.len() <= PRE_ROLL_CAP);
        u.set_clock_trusted(true);
        u.credit(NOON + 60, Activity::Active, 0);
        // The real day's hour plus every fake day's minute, all charged today.
        let expected = 3600 + 60 * rolls as u64;
        assert_eq!(u.used_today(NOON + 60), expected);
        assert!(u.pre_roll_days.is_empty(), "the chain is closed");
    }

    /// Restoring a snapshot never restores trust: a fresh ledger is untrusted
    /// until the caller says otherwise.
    #[test]
    fn a_restored_ledger_starts_untrusted() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.set_clock_trusted(true);
        u.credit(NOON, Activity::Active, 600);
        let far = NOON + 30 * 24 * 3600;
        u.credit(far, Activity::Active, 60);
        let mut r = UsageLedger::from_snapshot(&u.snapshot()).expect("restores");
        r.credit(NOON + 60, Activity::Active, 0);
        assert_eq!(r.current_day_key(NOON + 60), r.current_day_key(far));
    }

    /// The day-keyed side meters follow the same floor — otherwise a rollback
    /// would zero the journal and the learning/unrecognised counters while the
    /// scalar held, and the two would disagree about the same day.
    #[test]
    fn the_side_meters_hold_across_a_backwards_clock_step_too() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_bucket(NOON, Activity::Active, Bucket::Learning, 300);
        u.credit_unrecognised(NOON, Activity::Active, 120);
        u.credit_app_bucket(NOON, "play", 180);
        let yesterday = NOON - 24 * 3600;
        assert_eq!(u.learning_today_secs(yesterday), 300);
        assert_eq!(u.unrecognised_today_secs(yesterday), 120);
        assert_eq!(u.app_bucket_today_secs(yesterday, "play"), 180);
        assert_eq!(u.minutes_today(yesterday).count(), 5);
    }

    #[test]
    fn restart_persist_no_refill() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 1800);
        let snap = u.snapshot();
        let restored = UsageLedger::from_snapshot(&snap).unwrap();
        // A restart later the same day must NOT reset used_today.
        assert_eq!(restored.used_today(NOON + 60), 1800);
    }

    // NOON is 12:00 UTC = 13:00 local (BST, UTC+1): local minute 780. A span
    // of 120s ending then covers local minutes 778 and 779.
    #[test]
    fn active_screen_time_marks_minutes() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 120);
        let m = u.minutes_today(NOON);
        assert!(m.contains(778) && m.contains(779));
        assert!(!m.contains(780));
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn learning_marks_minutes_but_idle_does_not() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit_bucket(NOON, Activity::Active, Bucket::Learning, 60);
        assert_eq!(u.minutes_today(NOON).count(), 1); // learning IS screen time
        assert_eq!(u.used_today(NOON), 0); // ...but stays budget-free
        u.credit(NOON + 3600, Activity::Idle, 600);
        assert_eq!(u.minutes_today(NOON + 3600).count(), 1); // idle marks nothing
    }

    #[test]
    fn minutes_clear_on_day_roll() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 600);
        assert!(!u.minutes_today(NOON).is_empty());
        let next_day = NOON + 24 * 3600;
        // Reader alone already answers empty for the new day...
        assert!(u.minutes_today(next_day).is_empty());
        // ...and a credit on the new day rolls the journal for real.
        u.credit(next_day, Activity::Active, 60);
        assert_eq!(u.minutes_today(next_day).count(), 1);
        assert!(!u.minutes_today(next_day).contains(778));
    }

    #[test]
    fn snapshot_without_minutes_field_restores_empty() {
        let mut old = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        old.credit(NOON, Activity::Active, 1200);
        let mut v: serde_json::Value = serde_json::from_str(&old.snapshot()).unwrap();
        v.as_object_mut()
            .unwrap()
            .remove("minutes_today")
            .expect("field present in new snapshots");
        let restored = UsageLedger::from_snapshot(&v.to_string()).expect("restores");
        assert_eq!(restored.used_today(NOON), 1200);
        assert!(restored.minutes_today(NOON).is_empty());
    }

    #[test]
    fn minutes_survive_snapshot_roundtrip() {
        let mut u = UsageLedger::new(TZ, WeekStart::Mon, NOON);
        u.credit(NOON, Activity::Active, 120);
        let restored = UsageLedger::from_snapshot(&u.snapshot()).unwrap();
        assert_eq!(restored.minutes_today(NOON), u.minutes_today(NOON));
        assert_eq!(restored.minutes_today(NOON).count(), 2);
    }
}

#[cfg(test)]
mod bucket_meter_tests {
    use super::*;

    const TZ: &str = "Europe/London";
    // 2026-07-25 12:00 local.
    const NOON: i64 = 1785412800;

    fn ledger() -> UsageLedger {
        UsageLedger::new(TZ, WeekStart::Mon, NOON)
    }

    #[test]
    fn a_bucket_meter_accrues_and_reads_back() {
        let mut u = ledger();
        u.credit_app_bucket(NOON, "play", 600);
        u.credit_app_bucket(NOON, "play", 300);
        assert_eq!(u.app_bucket_today_secs(NOON, "play"), 900);
    }

    #[test]
    fn buckets_do_not_bleed_into_one_another() {
        let mut u = ledger();
        u.credit_app_bucket(NOON, "play", 600);
        assert_eq!(u.app_bucket_today_secs(NOON, "social"), 0);
    }

    /// The allowance is DAILY: a spent bucket must come back tomorrow, and a
    /// ledger read after midnight must not report yesterday's spend.
    #[test]
    fn the_allowance_returns_the_next_day() {
        let mut u = ledger();
        u.credit_app_bucket(NOON, "play", 3600);
        let next_day = NOON + 24 * 3600;
        assert_eq!(u.app_bucket_today_secs(next_day, "play"), 0);
        u.credit_app_bucket(next_day, "play", 60);
        assert_eq!(u.app_bucket_today_secs(next_day, "play"), 60);
        // …and yesterday's spend is genuinely gone, not merely hidden.
        assert_eq!(u.app_bucket_today_secs(next_day, "play"), 60);
    }

    /// Bucket time is ALSO screen time — the two meters move together. (This is
    /// what separates a capped bucket from the free learning bucket.)
    #[test]
    fn bucket_time_still_drains_the_day() {
        let mut u = ledger();
        u.credit_bucket(NOON, Activity::Active, Bucket::Screen, 600);
        u.credit_app_bucket(NOON, "play", 600);
        assert_eq!(u.app_bucket_today_secs(NOON, "play"), 600);
        assert_eq!(u.used_today(NOON), 600);
    }

    /// Restoring a ledger written before buckets existed must not explode or
    /// invent spend — the field defaults to empty.
    #[test]
    fn pre_buckets_snapshots_restore_empty() {
        let u = ledger();
        let mut json: serde_json::Value = serde_json::to_value(&u).unwrap();
        json.as_object_mut().unwrap().remove("bucket_today_secs");
        let back: UsageLedger = serde_json::from_value(json).unwrap();
        assert_eq!(back.app_bucket_today_secs(NOON, "play"), 0);
    }

    /// The week meter survives the day roll (it isn't a day-scoped counter)
    /// and only clears when the calendar week itself rolls over.
    #[test]
    fn bucket_week_meter_survives_the_day_roll_and_dies_at_the_week_roll() {
        let mut u = ledger(); // week_start Mon; exactly +7 days always crosses a week key.
        u.credit_app_bucket(NOON, "play", 1200);
        let tue = NOON + 24 * 3600;
        assert_eq!(u.app_bucket_today_secs(tue, "play"), 0); // day rolled
        assert_eq!(u.app_bucket_week_secs(tue, "play"), 1200); // week persists
        u.credit_app_bucket(tue, "play", 600);
        assert_eq!(u.app_bucket_week_secs(tue, "play"), 1800);
        let next_week = NOON + 7 * 24 * 3600;
        assert_eq!(u.app_bucket_week_secs(next_week, "play"), 0); // week rolled
    }

    /// Restoring a ledger written before weekly buckets existed must not
    /// explode or invent spend — the field defaults to empty (same idiom as
    /// `pre_buckets_snapshots_restore_empty`).
    #[test]
    fn pre_weekly_bucket_snapshots_restore_empty() {
        let mut u = ledger();
        u.credit_app_bucket(NOON, "play", 900);
        let mut json: serde_json::Value = serde_json::to_value(&u).unwrap();
        json.as_object_mut().unwrap().remove("bucket_week_secs");
        let back: UsageLedger = serde_json::from_value(json).unwrap();
        assert_eq!(back.app_bucket_week_secs(NOON, "play"), 0);
        // The day meter (present all along) is unaffected by the missing key.
        assert_eq!(back.app_bucket_today_secs(NOON, "play"), 900);
    }
}
