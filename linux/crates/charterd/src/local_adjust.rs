//! Time given or taken back **at the computer itself**, with no phone in the
//! loop — the standalone path.
//!
//! Kintrinsic's remote story (Kintrinsic over the relay) assumes the guardian has
//! their phone to hand and that the box is paired. Neither holds when someone
//! sets up a single family laptop with a screen-time limit and manages it from
//! that laptop: there is no `subject`, so the guardian-signed clause store
//! (where `gift` lives) has nowhere to put anything, and the guardian is
//! standing in front of the machine anyway.
//!
//! So a local adjustment is recorded per-uid under `/var/lib/charter/adjust/`,
//! written ONLY by the root helper `charter-time` (which the desktop reaches
//! through `pkexec`, so it costs an admin password — the sudo model), and
//! drained by the enforcement loop every tick.
//!
//! **This is deliberately not a clause and nothing signs it.** A clause is a
//! guardian's remote instruction that must be authenticated because it crossed
//! a network; this crossed a password prompt on the machine's own keyboard,
//! which is the stronger check of the two. Writing it as an unsigned local
//! record keeps that distinction legible instead of minting a fake signature.
//!
//! Idempotency is by entry id against the [`ExtensionLedger`]'s single applied
//! list, so re-reading the file every tick (which is exactly what the loop
//! does) applies each entry once, and a give and a take-back of the same size
//! cancel exactly rather than depending on which the loop saw first.

use serde::{Deserialize, Serialize};

use charter_schedule::enforcer::{LockReason, Remaining};
use charter_schedule::Dimension;
use charter_spine::multi_child::MultiChildEnforcer;

/// Where a child's local adjustments live. Root-owned; world-readable so the
/// ward's own console can show them (Kintrinsic's rule is that the ward can always
/// see what is being enforced), but only root can write.
pub fn adjust_path(uid: u32) -> String {
    std::env::var("CHARTER_ADJUST_DIR")
        .map(|d| format!("{d}/{uid}.json"))
        .unwrap_or_else(|_| format!("/var/lib/charter/adjust/{uid}.json"))
}

/// The frozen record version.
pub const ADJUST_VERSION: u32 = 1;

/// How long a spent entry is kept before the next write prunes it. Entries are
/// day-scoped by the ledger, so anything older than this can never apply
/// again; keeping two days means a box that was off overnight still finds
/// yesterday's ids in place and cannot re-apply them on a stale clock.
pub const ADJUST_RETENTION_SECS: i64 = 2 * 24 * 3600;

/// One give or take-back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustEntry {
    /// Unique per entry — the ledger's idempotency key.
    pub id: String,
    /// Positive = time given, negative = time taken back. Never zero.
    pub minutes: i32,
    /// When the admin made the change (unix seconds).
    pub at: i64,
    /// The admin account that authorised it, for the on-screen account of what
    /// happened. A local change with no name attached is exactly the kind of
    /// silent adjustment Kintrinsic exists to not do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// A child's local adjustment file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustRecord {
    pub v: u32,
    #[serde(default)]
    pub entries: Vec<AdjustEntry>,
}

impl Default for AdjustRecord {
    fn default() -> Self {
        AdjustRecord {
            v: ADJUST_VERSION,
            entries: Vec::new(),
        }
    }
}

impl AdjustRecord {
    /// Parse a record. Fail-SAFE for a *cap*: an unreadable or wrong-version
    /// file yields NO entries, so a corrupt file can never confiscate time the
    /// ward is entitled to — and equally can never hand out time nobody
    /// granted. Both directions fail to "nothing happened".
    pub fn parse(text: &str) -> AdjustRecord {
        match serde_json::from_str::<AdjustRecord>(text) {
            Ok(r) if r.v == ADJUST_VERSION => r,
            _ => AdjustRecord::default(),
        }
    }

    /// Load a child's record from disk (absent file = empty).
    pub fn load(uid: u32) -> AdjustRecord {
        match std::fs::read_to_string(adjust_path(uid)) {
            Ok(text) => AdjustRecord::parse(&text),
            Err(_) => AdjustRecord::default(),
        }
    }

    /// Drop entries too old to ever apply again, keeping the file bounded.
    pub fn pruned(mut self, now: i64) -> AdjustRecord {
        self.entries
            .retain(|e| now.saturating_sub(e.at) < ADJUST_RETENTION_SECS);
        self
    }

    /// The net minutes recorded for today, for the on-screen account of what
    /// the guardian has already done today. Purely informational — enforcement
    /// reads the ledger, not this.
    pub fn net_minutes_since(&self, since: i64) -> i32 {
        self.entries
            .iter()
            .filter(|e| e.at >= since)
            .map(|e| e.minutes)
            .sum()
    }
}

/// Which limit a local adjustment should move.
///
/// The rule is "whichever wall the ward is actually up against", because that
/// is the only one they can feel. Minutes routed to the other dimension land
/// in an isolated pool and change nothing the ward can see — the same trap the
/// remote `time.extend` router exists to avoid, and the reason a guardian's
/// "+30 minutes" past bedtime once bought nothing at all.
///
/// Applies in BOTH directions: whatever a give would extend is what a
/// take-back should shorten, so the two are exact inverses.
pub fn dimension_for(r: &Remaining) -> Dimension {
    // Locked: the daemon's own reason is authoritative — it already decided
    // which wall is the explanation the ward was given.
    if r.locked {
        return match r.reason {
            Some(LockReason::Schedule) => Dimension::Schedule,
            _ => Dimension::Budget,
        };
    }
    // Unlocked: the binding wall is the smaller of the two, treating -1
    // (unbounded) as "not binding". If BOTH are unbounded nothing is binding;
    // Budget is the harmless default (the caller refuses the change before it
    // gets here — see `nothing_to_adjust`).
    match (r.schedule_secs, r.budget_secs) {
        (s, b) if s >= 0 && b >= 0 => {
            if s < b {
                Dimension::Schedule
            } else {
                Dimension::Budget
            }
        }
        (s, _) if s >= 0 => Dimension::Schedule,
        _ => Dimension::Budget,
    }
}

/// Apply every entry in `record` to `uid`'s live ledger, returning the net
/// minutes NEWLY applied this pass (0 once the file has already been drained).
///
/// Lives here rather than inline in the enforcement loop so the rule can be
/// tested without standing up a daemon: the loop calls this once per child per
/// tick and does nothing else with the record.
///
/// Each entry is routed independently, re-reading `remaining` between entries,
/// because applying one can change which wall is binding for the next — a give
/// that reopens the budget must not leave the following take-back aimed at a
/// dimension nobody is up against.
pub fn apply_to(multi: &mut MultiChildEnforcer, uid: u32, now: i64, record: &AdjustRecord) -> i32 {
    let mut net = 0;
    for entry in &record.entries {
        let Some(rem) = multi.remaining(uid, now) else {
            continue; // uid not tracked (not set up yet) — nothing to move
        };
        let dim = dimension_for(&rem);
        let id = format!("local:{}", entry.id);
        // Bounded by the same ceiling a remote gift respects, so the local
        // door can never be the wider one.
        let magnitude = entry.minutes.unsigned_abs().min(u16::MAX as u32) as u16;
        if magnitude == 0 {
            continue;
        }
        let newly = if entry.minutes > 0 {
            multi.apply_extension(uid, now, &id, magnitude, dim)
        } else {
            multi.deduct_extension(uid, now, &id, magnitude, dim)
        };
        if newly {
            net += entry.minutes;
        }
    }
    net
}

/// True when the ward has no bounded limit at all, so there is nothing for a
/// take-back to come off.
///
/// Giving time to an unlimited ward is equally meaningless, but a *give* that
/// does nothing merely disappoints; a *take-back* that silently does nothing
/// leaves a guardian believing they have acted when they have not. The surface
/// refuses both and says why, rather than reporting a success it did not have.
pub fn nothing_to_adjust(r: &Remaining) -> bool {
    r.schedule_secs < 0 && r.budget_secs < 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rem(schedule: i64, budget: i64, locked: bool, reason: Option<LockReason>) -> Remaining {
        Remaining {
            effective_secs: schedule.min(budget),
            schedule_secs: schedule,
            budget_secs: budget,
            extension_secs: 0,
            locked,
            reason,
            next_open_secs: None,
            budget_day_secs: -1,
            budget_week_secs: -1,
        }
    }

    #[test]
    fn a_locked_ward_is_adjusted_on_the_wall_that_locked_them() {
        assert_eq!(
            dimension_for(&rem(0, 3600, true, Some(LockReason::Schedule))),
            Dimension::Schedule
        );
        assert_eq!(
            dimension_for(&rem(3600, 0, true, Some(LockReason::Budget))),
            Dimension::Budget
        );
    }

    #[test]
    fn an_unlocked_ward_is_adjusted_on_the_nearer_wall() {
        // Bedtime is 20 minutes away, budget has 3 hours: bedtime binds.
        assert_eq!(
            dimension_for(&rem(20 * 60, 3 * 3600, false, None)),
            Dimension::Schedule
        );
        // Budget runs out first.
        assert_eq!(
            dimension_for(&rem(3 * 3600, 20 * 60, false, None)),
            Dimension::Budget
        );
    }

    #[test]
    fn an_unbounded_dimension_never_binds() {
        // No schedule clause at all: the budget is the only real wall.
        assert_eq!(dimension_for(&rem(-1, 600, false, None)), Dimension::Budget);
        // No budget clause: the window is the only real wall.
        assert_eq!(
            dimension_for(&rem(600, -1, false, None)),
            Dimension::Schedule
        );
    }

    #[test]
    fn a_ward_with_no_limits_has_nothing_to_adjust() {
        assert!(nothing_to_adjust(&rem(-1, -1, false, None)));
        assert!(!nothing_to_adjust(&rem(-1, 600, false, None)));
        assert!(!nothing_to_adjust(&rem(600, -1, false, None)));
    }

    #[test]
    fn a_corrupt_or_future_record_yields_no_entries() {
        assert!(AdjustRecord::parse("not json at all").entries.is_empty());
        assert!(
            AdjustRecord::parse(r#"{"v":99,"entries":[{"id":"a","minutes":30,"at":1}]}"#)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn a_valid_record_roundtrips_and_prunes_by_age() {
        let now = 1_782_734_400;
        let r = AdjustRecord {
            v: ADJUST_VERSION,
            entries: vec![
                AdjustEntry {
                    id: "old".into(),
                    minutes: 30,
                    at: now - ADJUST_RETENTION_SECS - 1,
                    by: Some("decented".into()),
                },
                AdjustEntry {
                    id: "fresh".into(),
                    minutes: -20,
                    at: now - 60,
                    by: Some("decented".into()),
                },
            ],
        };
        let back = AdjustRecord::parse(&serde_json::to_string(&r).unwrap());
        assert_eq!(back, r);
        let pruned = back.pruned(now);
        assert_eq!(pruned.entries.len(), 1);
        assert_eq!(pruned.entries[0].id, "fresh");
    }

    #[test]
    fn the_net_for_today_is_the_sum_of_todays_entries() {
        let now = 1_782_734_400;
        let r = AdjustRecord {
            v: ADJUST_VERSION,
            entries: vec![
                AdjustEntry {
                    id: "yesterday".into(),
                    minutes: 60,
                    at: now - 30 * 3600,
                    by: None,
                },
                AdjustEntry {
                    id: "a".into(),
                    minutes: 30,
                    at: now - 600,
                    by: None,
                },
                AdjustEntry {
                    id: "b".into(),
                    minutes: -10,
                    at: now - 300,
                    by: None,
                },
            ],
        };
        assert_eq!(r.net_minutes_since(now - 24 * 3600), 20);
    }
}
