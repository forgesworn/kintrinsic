//! Failing SAFE when a child's usage ledger cannot be saved (R3-H1).
//!
//! The loop meters each child's time in memory and writes the ledger out
//! every slow tick, so a restart picks up where the day stood. A save that
//! keeps failing used to be logged and ignored — "still being enforced from
//! memory" — which holds only until the next reboot: the ledger then comes
//! back from the last snapshot that DID land, and everything spent since is
//! handed back.
//!
//! The motivating case is a lever the ward holds, not a failing disk. ext4
//! reserves BLOCKS for root (5% by default) but not INODES, and a default
//! install puts `/home` and `/var/lib` on one filesystem. An unprivileged
//! ward can create a few million empty files in their own home and exhaust
//! the inode table; every [`atomic_write`](crate::atomic_file::atomic_write)
//! needs a new inode for its temp file, so from then on every save fails
//! with `ENOSPC`, and a reboot is a refill — repeatable at will.
//!
//! So once a child's saves have failed CONTINUOUSLY for
//! [`USAGE_UNSAVED_AFTER_SECS`], their budget is treated as paused — the
//! same zero-a-day, zero-a-week fail-safe as an unreadable budget clause —
//! until a save lands again, and STATUS says so (`usageUnsaved`). A brief
//! blip (a full disk cleared, a slow mount) never touches enforcement.
//!
//! Except when the save fails for want of space or inodes (`ENOSPC`,
//! `EDQUOT`): that is the ward's lever above, not a blip, so it trips at once
//! (R4-1). The five-minute grace lives only in memory, so it restarts on
//! every boot — a ward holding the inode table full and rebooting every five
//! minutes would otherwise get most of each day back in five-minute chunks.
//! With the immediate trip the most a reboot gives back is the time charged
//! before the first save of the run, and that save runs on the loop's first
//! iteration (see `runtime.rs`, step 4). Other errors (`EIO`, a read-only
//! remount) keep the five-minute continuous window, so a transient fault on a
//! legitimate child's disk never locks them.
//!
//! Scope (R4-2): each child has their own ledger file, and the guard is kept
//! per child. The pause applies only to a child with a budget in force —
//! a schedule-only child has no daily cap for a reboot to refill, so
//! `MultiChildEnforcer::set_usage_unsaved` leaves them alone. Inode
//! exhaustion is filesystem-wide, so in practice every budgeted child's save
//! fails together and every budgeted sibling is paused with the ward who
//! caused it; that is the fail-safe direction and deliberate.
//!
//! Lifting it (R4-3): only a save that lands, a stand-down, or the admin
//! pause flag. Under inode exhaustion the admin cannot `touch` the pause flag
//! either (a new file needs a free inode), so they must free inodes first —
//! delete the ward's flood of files — which is also what lets saves land.
//!
//! Timed on the MONOTONIC clock the caller supplies, never the wall clock:
//! the ward can move the wall clock, and a clock stepped back would
//! otherwise hold the timer short of tripping for as long as they liked.

use std::collections::{BTreeMap, BTreeSet};

/// How long a child's usage-ledger saves must have failed without a break
/// before their budget is held paused.
pub const USAGE_UNSAVED_AFTER_SECS: u64 = 5 * 60;

/// A change worth one log line. Everything else is silent: a save failing
/// every ten seconds would otherwise fill the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTransition {
    /// The first failure after a success (or since start): the error.
    StartedFailing(String),
    /// Failing for [`USAGE_UNSAVED_AFTER_SECS`]: the budget is now paused.
    Tripped,
    /// Failed for want of space or inodes (`ENOSPC`/`EDQUOT`): paused at
    /// once, without the grace window (R4-1). Carries the error.
    TrippedStorageFull(String),
    /// A save landed after a failing stretch; `was_tripped` says whether the
    /// budget had been paused and is now released.
    Recovered { was_tripped: bool },
}

/// Per-child record of how long the usage-ledger saves have been failing.
#[derive(Debug, Default)]
pub struct UsageSaveGuard {
    /// Monotonic seconds of the first failure in each child's current
    /// failing stretch. Absent = the last save landed.
    failing_since: BTreeMap<u32, u64>,
    /// Children whose stretch has passed the threshold.
    tripped: BTreeSet<u32>,
}

/// `ENOSPC` or `EDQUOT`: out of blocks, inodes or quota — the failure an
/// unprivileged ward can cause at will (inode exhaustion), so it trips the
/// guard at once. Matched by kind and by the Linux errno both, so the check
/// does not hang on how the standard library maps the code.
fn is_storage_full(e: &std::io::Error) -> bool {
    /// Linux `ENOSPC`.
    const ENOSPC: i32 = 28;
    /// Linux `EDQUOT`.
    const EDQUOT: i32 = 122;
    matches!(
        e.kind(),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
    ) || matches!(e.raw_os_error(), Some(ENOSPC | EDQUOT))
}

impl UsageSaveGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one save attempt for `uid` at monotonic second `mono_secs`.
    /// Returns the transition, if any, for the caller to log.
    pub fn record(
        &mut self,
        uid: u32,
        result: &std::io::Result<()>,
        mono_secs: u64,
    ) -> Option<SaveTransition> {
        match result {
            Ok(()) => {
                let was_failing = self.failing_since.remove(&uid).is_some();
                let was_tripped = self.tripped.remove(&uid);
                was_failing.then_some(SaveTransition::Recovered { was_tripped })
            }
            Err(e) if is_storage_full(e) => {
                self.failing_since.entry(uid).or_insert(mono_secs);
                self.tripped
                    .insert(uid)
                    .then(|| SaveTransition::TrippedStorageFull(e.to_string()))
            }
            Err(e) => {
                let since = match self.failing_since.get(&uid) {
                    Some(s) => *s,
                    None => {
                        self.failing_since.insert(uid, mono_secs);
                        return Some(SaveTransition::StartedFailing(e.to_string()));
                    }
                };
                if mono_secs.saturating_sub(since) >= USAGE_UNSAVED_AFTER_SECS
                    && self.tripped.insert(uid)
                {
                    return Some(SaveTransition::Tripped);
                }
                None
            }
        }
    }

    /// Whether `uid`'s budget must be held paused right now.
    pub fn is_unsaved(&self, uid: u32) -> bool {
        self.tripped.contains(&uid)
    }

    /// Forget every child not in `uids` (no longer managed).
    pub fn retain(&mut self, uids: &[u32]) {
        self.failing_since.retain(|u, _| uids.contains(u));
        self.tripped.retain(|u| uids.contains(u));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_file::atomic_write;
    use crate::multi_child::MultiChildEnforcer;
    use charter_spine::local_limits::DeviceLimits;

    const KID: u32 = 1001;
    const OTHER: u32 = 1002;

    /// A ledger path whose "directory" is a regular file: every write fails
    /// (`ENOTDIR`) until the file is removed — a filesystem that refuses
    /// writes, without needing root to make one.
    struct FailingDir {
        root: std::path::PathBuf,
        blocker: std::path::PathBuf,
    }

    impl FailingDir {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "charterd-usage-guard-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let blocker = root.join("ledgers");
            std::fs::write(&blocker, b"not a directory").unwrap();
            FailingDir { root, blocker }
        }
        fn path(&self) -> String {
            self.blocker
                .join("usage-1001.json")
                .to_string_lossy()
                .into_owned()
        }
        fn heal(&self) {
            std::fs::remove_file(&self.blocker).unwrap();
        }
    }

    impl Drop for FailingDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn enforcer(now: i64) -> MultiChildEnforcer {
        let limits = |m| DeviceLimits {
            tz: "UTC".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: m,
            weekend: None,
        };
        let pol = |m| {
            crate::child_policy::resolve_effective(
                &charter_sys::persistence::ChildClauses::default(),
                Some(&limits(m)),
                None,
            )
        };
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(KID, pol(60)), (OTHER, pol(60))], now, |_| (None, None));
        e
    }

    /// Five minutes of failed writes pause the child's budget; one that
    /// lands releases it. Driven as the loop does: a save every slow tick,
    /// the guard's verdict handed to the enforcer.
    #[test]
    fn five_minutes_of_failed_saves_pause_the_budget_until_one_lands() {
        let fs = FailingDir::new("trip");
        // A Wednesday, 10:00 UTC — inside the 07:00–20:00 window.
        let now: i64 = 1_700_042_400;
        let mut e = enforcer(now);
        let mut g = UsageSaveGuard::new();
        let mut transitions = Vec::new();
        let mut tick = |e: &mut MultiChildEnforcer, g: &mut UsageSaveGuard, mono: u64| {
            let snap = e.snapshots();
            let (_, usage, _) = snap.iter().find(|(u, _, _)| *u == KID).unwrap();
            let r = atomic_write(&fs.path(), usage.as_bytes(), 0o600);
            if let Some(t) = g.record(KID, &r, mono) {
                transitions.push(t);
            }
            e.set_usage_unsaved(KID, g.is_unsaved(KID));
            e.tick(Some(KID), now + mono as i64, 10);
        };
        // Four minutes fifty of failures: not yet.
        for mono in (0..300).step_by(10) {
            tick(&mut e, &mut g, mono);
        }
        assert!(!g.is_unsaved(KID));
        assert!(!e.remaining(KID, now + 300).unwrap().locked);
        // Five minutes: paused, locked, nothing left. The other child is not.
        tick(&mut e, &mut g, 300);
        assert!(g.is_unsaved(KID));
        let r = e.remaining(KID, now + 300).unwrap();
        assert!(r.locked);
        assert_eq!(r.budget_secs, 0);
        assert!(!e.usage_unsaved(OTHER));
        // Staying failed does not log again.
        for mono in (310..400).step_by(10) {
            tick(&mut e, &mut g, mono);
        }
        // The filesystem recovers: the next save lands and releases it.
        fs.heal();
        tick(&mut e, &mut g, 400);
        assert!(!g.is_unsaved(KID));
        assert!(!e.remaining(KID, now + 400).unwrap().locked);
        assert!(std::fs::metadata(fs.path()).is_ok(), "the ledger landed");
        assert_eq!(
            transitions.len(),
            3,
            "log on transition only: {transitions:?}"
        );
        assert!(matches!(transitions[0], SaveTransition::StartedFailing(_)));
        assert_eq!(transitions[1], SaveTransition::Tripped);
        assert_eq!(
            transitions[2],
            SaveTransition::Recovered { was_tripped: true }
        );
    }

    /// A success anywhere in the stretch restarts the clock: only a
    /// CONTINUOUS five minutes of failure trips.
    #[test]
    fn an_intermittent_failure_never_trips() {
        let mut g = UsageSaveGuard::new();
        let err = || Err(std::io::Error::from_raw_os_error(5)); // EIO
        for mono in (0..3000).step_by(10) {
            let r = if mono % 240 == 0 { Ok(()) } else { err() };
            g.record(KID, &r, mono);
            assert!(!g.is_unsaved(KID), "at {mono}");
        }
        // A brief stretch that recovers logs as recovered-without-trip.
        assert!(g.record(OTHER, &err(), 0).is_some());
        assert_eq!(
            g.record(OTHER, &Ok(()), 10),
            Some(SaveTransition::Recovered { was_tripped: false })
        );
        assert_eq!(g.record(OTHER, &Ok(()), 20), None);
    }

    #[test]
    fn retain_forgets_children_no_longer_managed() {
        let mut g = UsageSaveGuard::new();
        let err = || Err(std::io::Error::from_raw_os_error(5)); // EIO
        g.record(KID, &err(), 0);
        g.record(KID, &err(), USAGE_UNSAVED_AFTER_SECS);
        assert!(g.is_unsaved(KID));
        g.retain(&[OTHER]);
        assert!(!g.is_unsaved(KID));
        // And a returning child starts a fresh stretch.
        assert!(matches!(
            g.record(KID, &err(), 1000),
            Some(SaveTransition::StartedFailing(_))
        ));
    }

    /// R4-1: out of space, inodes or quota trips on the FIRST failed save —
    /// no five-minute grace for a reboot loop to harvest — and one save that
    /// lands still releases it.
    #[test]
    fn a_storage_full_failure_trips_at_once() {
        for code in [28, 122] {
            let mut g = UsageSaveGuard::new();
            let r = Err(std::io::Error::from_raw_os_error(code));
            assert!(
                matches!(
                    g.record(KID, &r, 0),
                    Some(SaveTransition::TrippedStorageFull(_))
                ),
                "errno {code}"
            );
            assert!(g.is_unsaved(KID), "errno {code}");
            // Staying failed does not log again.
            assert_eq!(g.record(KID, &r, 10), None);
            assert_eq!(
                g.record(KID, &Ok(()), 20),
                Some(SaveTransition::Recovered { was_tripped: true })
            );
            assert!(!g.is_unsaved(KID));
        }
        // By kind alone, too (no raw errno attached).
        let mut g = UsageSaveGuard::new();
        for kind in [
            std::io::ErrorKind::StorageFull,
            std::io::ErrorKind::QuotaExceeded,
        ] {
            g.record(KID, &Ok(()), 0);
            g.record(KID, &Err(std::io::Error::from(kind)), 0);
            assert!(g.is_unsaved(KID), "{kind:?}");
        }
        // A stretch that began with another error trips as soon as it turns
        // into ENOSPC, well inside the window.
        let mut g = UsageSaveGuard::new();
        g.record(OTHER, &Err(std::io::Error::from_raw_os_error(5)), 0);
        assert!(!g.is_unsaved(OTHER));
        g.record(OTHER, &Err(std::io::Error::from_raw_os_error(28)), 10);
        assert!(g.is_unsaved(OTHER));
    }

    /// Any other error still needs the full continuous window.
    #[test]
    fn other_errors_keep_the_five_minute_window() {
        let mut g = UsageSaveGuard::new();
        let eio = || Err(std::io::Error::from_raw_os_error(5));
        assert!(matches!(
            g.record(KID, &eio(), 0),
            Some(SaveTransition::StartedFailing(_))
        ));
        assert_eq!(g.record(KID, &eio(), USAGE_UNSAVED_AFTER_SECS - 1), None);
        assert!(!g.is_unsaved(KID));
        assert_eq!(
            g.record(KID, &eio(), USAGE_UNSAVED_AFTER_SECS),
            Some(SaveTransition::Tripped)
        );
    }

    /// R4-2: a child with no budget in force is never paused by a failing
    /// ledger — there is no cap for a reboot to refill — while a budgeted
    /// sibling is.
    #[test]
    fn a_child_without_a_budget_is_never_paused() {
        let now: i64 = 1_700_042_400; // Wednesday 10:00 UTC
        let limits = DeviceLimits {
            tz: "UTC".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 60,
            weekend: None,
        };
        let budgeted = crate::child_policy::resolve_effective(
            &charter_sys::persistence::ChildClauses::default(),
            Some(&limits),
            None,
        );
        let mut schedule_only = budgeted.clone();
        schedule_only.budget = None;
        let mut e = MultiChildEnforcer::new();
        e.sync(&[(KID, budgeted), (OTHER, schedule_only)], now, |_| {
            (None, None)
        });
        let mut g = UsageSaveGuard::new();
        let enospc = || Err(std::io::Error::from_raw_os_error(28));
        for uid in [KID, OTHER] {
            g.record(uid, &enospc(), 0);
            e.set_usage_unsaved(uid, g.is_unsaved(uid));
        }
        let d = e.tick(Some(OTHER), now + 10, 10);
        let locked = |uid| d.iter().find(|x| x.uid == uid).unwrap().locked;
        assert!(locked(KID), "the budgeted child is paused");
        assert!(e.usage_unsaved(KID));
        assert!(!locked(OTHER), "the schedule-only child is not");
        assert!(!e.usage_unsaved(OTHER));
    }
}
