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
    }
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
}
