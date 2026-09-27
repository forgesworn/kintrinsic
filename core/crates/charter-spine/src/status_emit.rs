//! Building + throttling the device STATUS feed (kind 31114). Pure glue from a
//! child's per-tick enforcement state to the wire `StatusPayload`; the emit
//! itself (gift-wrap + relay publish) is real-only, in `runtime.rs`.

use charter_primitives::PubKey;
use charter_proto::{StatusLockReason, StatusPayload, StatusSource, STATUS_VERSION};
use charter_schedule::{LockReason, Remaining};

use crate::child_policy::PolicySource;

fn to_status_source(s: PolicySource) -> StatusSource {
    match s {
        PolicySource::Guardian => StatusSource::Guardian,
        PolicySource::DeviceOnly => StatusSource::DeviceOnly,
        PolicySource::Unconstrained => StatusSource::Unconstrained,
    }
}

fn to_status_lock_reason(r: LockReason) -> StatusLockReason {
    match r {
        LockReason::Schedule => StatusLockReason::Schedule,
        LockReason::Budget => StatusLockReason::Budget,
        LockReason::Malformed => StatusLockReason::Malformed,
        LockReason::StandDown => StatusLockReason::StandDown,
    }
}

/// Build the STATUS payload for one child from their tick state. `used_*` are the
/// device's raw usage (the multi-device aggregation inputs); the display fields
/// come from `remaining`. Negative "left" values saturate to 0.
#[allow(clippy::too_many_arguments)]
pub fn build_status(
    subject: PubKey,
    machine: PubKey,
    ts: u64,
    remaining: &Remaining,
    used_today_secs: u64,
    used_week_secs: Option<u64>,
    day_key: String,
    week_key: Option<String>,
    source: PolicySource,
) -> StatusPayload {
    StatusPayload {
        v: STATUS_VERSION,
        subject,
        machine,
        ts,
        day_key,
        week_key,
        used_today_secs,
        used_week_secs,
        // Stamped by the caller after building, like learning_today_secs: the
        // builder already carries a too_many_arguments waiver and these are
        // display-only.
        site_runtime_missing: None,
        daily_minutes: None,
        weekly_minutes: None,
        window_left_secs: remaining.schedule_secs.max(0) as u64,
        quota_left_secs: remaining.budget_secs.max(0) as u64,
        effective_secs: remaining.effective_secs.max(0) as u64,
        locked: remaining.locked,
        lock_reason: remaining.reason.map(to_status_lock_reason),
        source: to_status_source(source),
        // The QR-onboarding echo is a transport-layer concern; the warden that
        // holds a fresh token stamps it on the built payload itself.
        pair_token: None,
        apps: None,
        // Stamped by the loop when a learning clause is in force (same
        // pattern as pair_token).
        learning_today_secs: None,
        // Stamped by the device loop (same pattern as pair_token/apps).
        app_version_code: None,
        app_version_name: None,
        update_health: None,
        // Stamped by the device loop from the maintenance-span slot: an
        // account of the last window a guardian opened.
        install_window: None,
        // Stamped by the loop from the ledger's minute journal (B3).
        active_minutes_today: None,
        // Stamped by the loop from the bucket meters when a `buckets` clause
        // is in force (same pattern as pair_token/apps).
        groups: None,
        // Stamped by the loop from the unrecognised-time meter (same pattern
        // as pair_token/apps) — absent unless it is actually non-zero.
        unrecognised_today_secs: None,
        // Stamped by the loop from the out-of-hours meter (Android only —
        // the always-available clause that produces it doesn't exist on
        // Linux), same pattern as unrecognised_today_secs in reverse.
        out_of_hours_today_secs: None,
        out_of_hours_week_secs: None,
        out_of_hours_nights_week: None,
        // Stamped by the loop from the boot watch (Android only — safe mode
        // is the gap this counts, and only Android has one). Absent unless
        // the device has actually booted without its warden.
        enforcement_gap: None,
        // The four below are stamped by the device loop from runtime/state-
        // file data (03-G5/04-G6/B4/relay-health follow-up), same pattern as
        // pair_token/apps.
        paused_by_admin: None,
        enforcement_gap_secs: None,
        relay_unreachable_polls: None,
        transport_unavailable: None,
        // Stamped by the loop with [`clock_stepped_back_from`] against the
        // last EMITTED status, which only the loop holds.
        clock_stepped_back_from: None,
        // Stamped by the loop from its [`StatusSeq`] as the status is
        // emitted, once the new value is durable.
        seq: None,
        // Stamped by the loop, which alone sees whether the usage ledger's
        // saves are landing.
        usage_unsaved: None,
    }
}

/// The device's STATUS `seq` counter (R2-2/R2-3): strictly increasing per
/// device, across restarts, whatever the wall clock does.
///
/// The platform owns the storage; this owns the arithmetic. At start the
/// platform reads its stored value (`None` when missing or unreadable) and
/// calls [`resume`](Self::resume); for each STATUS it emits it takes
/// [`advance`](Self::advance), stores that value durably (atomically) and only then
/// stamps and publishes it. A value consumed by an emit that never reached a
/// relay is simply a gap: the guardian needs order, not density.
///
/// Seeding takes the higher of the stored value and the wall clock in unix
/// MILLISECONDS. With a stored value the first `next` is therefore never
/// below stored + 1; without one (a first start, a wiped or torn file) the
/// sequence restarts from the clock, which on a sane clock is far above any
/// value the old sequence reached — one emit per heartbeat never gains on a
/// counter that advances a thousand a second. A clock the ward has set back
/// can make a WIPED sequence restart low, but the state is root's, so the ward
/// cannot wipe it; and a stored value always wins over a low clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusSeq {
    last: u64,
}

impl StatusSeq {
    /// Resume from the stored value (`None` if missing or unreadable) and the
    /// wall clock now, in unix milliseconds.
    pub fn resume(stored: Option<u64>, now_unix_ms: u64) -> Self {
        StatusSeq {
            last: stored.unwrap_or(0).max(now_unix_ms),
        }
    }

    /// The next `seq` to stamp. Store it durably before publishing.
    pub fn advance(&mut self) -> u64 {
        self.last = self.last.saturating_add(1);
        self.last
    }

    /// Parse a stored value: one decimal `u64`, surrounding whitespace
    /// ignored. Anything else is unreadable (`None`), which [`resume`]
    /// answers from the clock.
    pub fn parse_stored(s: &str) -> Option<u64> {
        s.trim().parse().ok()
    }

    /// The stored form of `seq`.
    pub fn format_stored(seq: u64) -> String {
        format!("{seq}\n")
    }
}

/// The `clockSteppedBackFrom` value for a STATUS about to be emitted at
/// `ts`, given the last STATUS emitted for the same child (`last`).
///
/// When the wall clock now reads below `last.ts` this is `last.ts`: the one
/// status that tells the guardian "I saw my clock step back from here", so it
/// may accept a lower-`ts` status as current. Otherwise it is `None`, so the
/// statuses after that first one omit it — except that a stamp the loop
/// could not deliver (`undelivered`, the stamp of an emit that reached no
/// relay) is carried until one does. Without the carry a failed publish would
/// lose the only stamp, and the guardian would treat the feed as stale until
/// wall time climbed back past its stored `ts`. When both apply the higher
/// wins: the guardian's stored `ts` is at most the higher of the two.
///
/// Held in memory only, like the last-emit `ts` it is computed from: after a
/// restart the first status carries no stamp. Ordering does not depend on it
/// any more — a guardian orders by the durable [`StatusSeq`] when a status
/// carries one — so all a lost marker costs is the "clock went backwards"
/// notice.
pub fn clock_stepped_back_from(
    last: Option<&StatusPayload>,
    ts: u64,
    undelivered: Option<u64>,
) -> Option<u64> {
    let stepped = last.filter(|p| ts < p.ts).map(|p| p.ts);
    stepped.max(undelivered)
}

/// Whether to emit STATUS this tick: on any displayable **state** change
/// (locked / source / lock reason), or after `heartbeat_secs` of clock
/// movement in EITHER direction since the last emit (so a steady state still
/// refreshes the PWA). The continuously-ticking time fields are deliberately
/// NOT compared — else every tick would publish.
///
/// The comparison is an absolute difference, and a clock that has moved
/// BACKWARDS at all forces an emit. With a saturating forward-only subtraction,
/// a backwards step — an NTP correction, a suspend/resume glitch, a ward who
/// got at the clock — silenced the feed until wall time climbed back past the
/// last emit's `ts`: set the clock back a week and the guardian's feed went
/// quiet for a week while the PWA kept showing the last known state as if it
/// were current. Silence is the one failure the guardian cannot see.
pub fn should_emit_status(
    last: Option<&StatusPayload>,
    cur: &StatusPayload,
    heartbeat_secs: u64,
) -> bool {
    match last {
        None => true,
        Some(prev) => {
            prev.locked != cur.locked
                || prev.source != cur.source
                || prev.lock_reason != cur.lock_reason
                // A boot the warden slept through is news, and news should not
                // wait out a heartbeat. It changes at most once per boot, so
                // it can never become a source of churn.
                || prev.enforcement_gap != cur.enforcement_gap
                || cur.ts < prev.ts
                || cur.ts.abs_diff(prev.ts) >= heartbeat_secs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remaining(locked: bool, reason: Option<LockReason>) -> Remaining {
        Remaining {
            effective_secs: 900,
            schedule_secs: 1800,
            budget_secs: 900,
            extension_secs: 0,
            locked,
            reason,
            next_open_secs: None,
            budget_day_secs: -1,
            budget_week_secs: -1,
        }
    }

    #[test]
    fn build_status_maps_display_fields_and_enums() {
        let r = remaining(true, Some(LockReason::Budget));
        let s = build_status(
            PubKey::from_bytes([1; 32]),
            PubKey::from_bytes([2; 32]),
            100,
            &r,
            600,
            Some(1200),
            "2026-07-02".into(),
            Some("2026-W27".into()),
            PolicySource::DeviceOnly,
        );
        assert_eq!(s.window_left_secs, 1800);
        assert_eq!(s.quota_left_secs, 900);
        assert_eq!(s.effective_secs, 900);
        assert!(s.locked);
        assert_eq!(s.lock_reason, Some(StatusLockReason::Budget));
        assert_eq!(s.source, StatusSource::DeviceOnly);
        assert_eq!(s.used_today_secs, 600);
        assert_eq!(s.used_week_secs, Some(1200));
    }

    #[test]
    fn build_status_saturates_negative_left_to_zero() {
        let mut r = remaining(false, None);
        r.schedule_secs = -5;
        r.budget_secs = -1;
        r.effective_secs = -10;
        let s = build_status(
            PubKey::from_bytes([1; 32]),
            PubKey::from_bytes([2; 32]),
            1,
            &r,
            0,
            None,
            "d".into(),
            None,
            PolicySource::Guardian,
        );
        assert_eq!(s.window_left_secs, 0);
        assert_eq!(s.quota_left_secs, 0);
        assert_eq!(s.effective_secs, 0);
    }

    #[test]
    fn throttle_emits_on_state_change_and_heartbeat_not_every_tick() {
        let r = remaining(false, None);
        let base = build_status(
            PubKey::from_bytes([1; 32]),
            PubKey::from_bytes([2; 32]),
            100,
            &r,
            0,
            None,
            "d".into(),
            None,
            PolicySource::Guardian,
        );
        // First ever → emit.
        assert!(should_emit_status(None, &base, 60));
        // Same state, within the heartbeat (time still ticking) → no emit.
        let mut later = base.clone();
        later.ts = 130;
        assert!(!should_emit_status(Some(&base), &later, 60));
        // Heartbeat elapsed → emit.
        let mut hb = base.clone();
        hb.ts = 161;
        assert!(should_emit_status(Some(&base), &hb, 60));
        // A lock transition → emit immediately, even within the heartbeat.
        let mut locked = base.clone();
        locked.ts = 105;
        locked.locked = true;
        locked.lock_reason = Some(StatusLockReason::Schedule);
        assert!(should_emit_status(Some(&base), &locked, 60));
    }

    /// B7: a backwards clock step must not silence the feed. With a
    /// forward-only saturating subtraction, setting the clock back a week left
    /// the guardian looking at a week-old state presented as current.
    #[test]
    fn a_backwards_clock_step_still_emits() {
        let r = remaining(false, None);
        let base = build_status(
            PubKey::from_bytes([1; 32]),
            PubKey::from_bytes([2; 32]),
            1_000_000,
            &r,
            0,
            None,
            "d".into(),
            None,
            PolicySource::Guardian,
        );
        // A week backwards: the old comparison saturated to 0 and stayed quiet
        // until wall time climbed back past 1_000_000.
        let mut back = base.clone();
        back.ts = 1_000_000 - 7 * 86_400;
        assert!(should_emit_status(Some(&base), &back, 60));
        // Even one second backwards is news — the clock moved under us.
        let mut nudged = base.clone();
        nudged.ts = 999_999;
        assert!(should_emit_status(Some(&base), &nudged, 60));
        // Forward within the heartbeat is still throttled.
        let mut fwd = base.clone();
        fwd.ts = 1_000_030;
        assert!(!should_emit_status(Some(&base), &fwd, 60));
    }

    #[test]
    fn status_seq_is_strictly_increasing_and_never_below_stored_plus_one() {
        // Stored value ahead of the clock (a clock set back): stored + 1.
        let mut s = StatusSeq::resume(Some(5_000_000_000_000), 1_000);
        assert_eq!(s.advance(), 5_000_000_000_001);
        assert_eq!(s.advance(), 5_000_000_000_002);
        // Stored value behind the clock: the clock wins, still above stored.
        let mut s = StatusSeq::resume(Some(10), 1_790_000_000_000);
        assert_eq!(s.advance(), 1_790_000_000_001);
    }

    #[test]
    fn status_seq_seeds_from_the_clock_when_nothing_is_stored() {
        let mut s = StatusSeq::resume(None, 1_790_000_000_000);
        let first = s.advance();
        assert_eq!(first, 1_790_000_000_001);
        // A wiped state a minute later, after an hour of heartbeats, still
        // sorts after everything the old sequence emitted.
        let mut old = StatusSeq::resume(None, 1_790_000_000_000);
        let mut last_old = 0;
        for _ in 0..60 {
            last_old = old.advance();
        }
        let mut wiped = StatusSeq::resume(None, 1_790_000_060_000);
        assert!(wiped.advance() > last_old);
    }

    #[test]
    fn status_seq_round_trips_through_storage_across_a_restart() {
        let mut s = StatusSeq::resume(None, 1_000);
        let a = s.advance();
        let stored = StatusSeq::format_stored(a);
        // Restart with the clock set back to the epoch.
        let mut r = StatusSeq::resume(StatusSeq::parse_stored(&stored), 0);
        assert!(r.advance() > a);
        assert_eq!(StatusSeq::parse_stored("garbage"), None);
        assert_eq!(StatusSeq::parse_stored(""), None);
        assert_eq!(StatusSeq::parse_stored("-3"), None);
    }

    #[test]
    fn build_status_leaves_seq_for_the_loop() {
        assert_eq!(at(1).seq, None);
    }

    fn at(ts: u64) -> StatusPayload {
        let mut s = build_status(
            PubKey::from_bytes([1; 32]),
            PubKey::from_bytes([2; 32]),
            ts,
            &remaining(false, None),
            0,
            None,
            "d".into(),
            None,
            PolicySource::Guardian,
        );
        s.clock_stepped_back_from = None;
        s
    }

    /// The first status after the clock steps back names the `ts` it stepped
    /// back from; the ones after it do not.
    #[test]
    fn a_backwards_step_is_stamped_once_with_the_previous_ts() {
        let first = at(1_000_000);
        assert_eq!(clock_stepped_back_from(None, 1_000_000, None), None);
        // Forward: nothing to say.
        assert_eq!(clock_stepped_back_from(Some(&first), 1_000_060, None), None);
        // Back two days: the previous emitted ts.
        let back_ts = 1_000_000 - 2 * 86_400;
        let mut back = at(back_ts);
        back.clock_stepped_back_from = clock_stepped_back_from(Some(&first), back_ts, None);
        assert_eq!(back.clock_stepped_back_from, Some(1_000_000));
        assert!(should_emit_status(Some(&first), &back, 60));
        assert!(back.to_json().contains("\"clockSteppedBackFrom\":1000000"));
        // The next status, measured against the stamped one, omits it.
        assert_eq!(
            clock_stepped_back_from(Some(&back), back_ts + 60, None),
            None
        );
        let mut next = at(back_ts + 60);
        next.clock_stepped_back_from = clock_stepped_back_from(Some(&back), next.ts, None);
        assert!(!next.to_json().contains("clockSteppedBackFrom"));
    }

    /// A stamp that reached no relay is carried to the next emit, and the
    /// higher of a carried stamp and a fresh step wins.
    #[test]
    fn an_undelivered_stamp_is_carried_until_it_lands() {
        let back = at(900_000);
        assert_eq!(
            clock_stepped_back_from(Some(&back), 900_060, Some(1_000_000)),
            Some(1_000_000)
        );
        // A second step back below a status that itself never landed.
        assert_eq!(
            clock_stepped_back_from(Some(&back), 800_000, Some(1_000_000)),
            Some(1_000_000)
        );
        let high = at(1_100_000);
        assert_eq!(
            clock_stepped_back_from(Some(&high), 800_000, Some(1_000_000)),
            Some(1_100_000)
        );
    }
}
