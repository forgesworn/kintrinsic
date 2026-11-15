//! The guardian screen-lock reset, ward side (`spec/contract.md`, "Screen-lock
//! reset"). The core authenticates a reset (`charter_verify::screen_lock`);
//! this module keeps what the ward must remember about it:
//!
//! - the [`Ledger`]: one outcome per reset id in a ring of the last
//!   [`RING_LEN`], the ids of a further [`FORGOTTEN_LEN`] that have left it,
//!   and the monotonic floor (the `issuedAt` of the last accepted reset).
//!   Persisted under `<base>/screen-lock/`, in
//!   device-protected storage and OUTSIDE everything a RELEASE clears, so a
//!   release cannot reopen a replay window or erase what the guardian is
//!   told;
//! - the [`Pending`] directive Kotlin takes: in memory ONLY, because it holds
//!   the PIN. A crash before Kotlin takes it loses the reset, which is the
//!   safe direction: the guardian sees no result and sends again.
//!
//! The PIN is never written to disk, never logged, and never put in STATUS
//! or AUDIT. [`Pending`] has a redacted `Debug` and wipes its PIN on drop.

use std::path::{Path, PathBuf};

use charter_proto::{ScreenLockResetOutcome, ScreenLockResetPayload, ScreenLockResetResult};
use charter_verify::ScreenLockResetError;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// How many reset outcomes the ward remembers (and STATUS carries).
pub const RING_LEN: usize = charter_proto::MAX_STATUS_SCREEN_LOCK_RESETS;

/// How many ids that have left the ring are still known as answered. The
/// relays re-deliver every wrap for about two days, so an id the ring drops
/// can come back; with this many more remembered, a re-delivery stays a no-op
/// unless 128 resets arrive inside that window.
pub const FORGOTTEN_LEN: usize = 112;

/// The AUDIT `op` for a reset's outcome.
pub const AUDIT_OP: &str = "screenlock.reset";

/// One reset id's recorded outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RingEntry {
    pub id: String,
    pub result: ScreenLockResetResult,
    /// When this result was recorded (device clock).
    pub at: u64,
}

/// The ward's durable memory of screen-lock resets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    /// `issuedAt` of the last ACCEPTED reset (0 before any): a new one must
    /// be strictly above it.
    pub floor: u64,
    /// Newest recorded first; at most [`RING_LEN`].
    pub ring: Vec<RingEntry>,
    /// The ids that have left the ring, most recently dropped first; at most
    /// [`FORGOTTEN_LEN`]. Their outcomes are gone, but they are still
    /// answered: a re-delivery of one changes nothing.
    #[serde(default)]
    pub forgotten: Vec<String>,
}

/// What [`Ledger::admit`] decided for an authenticated reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Its id has already been answered: a re-delivery. Nothing changed.
    Duplicate,
    /// Recorded `rejected_time`.
    RejectedTime,
    /// Recorded `superseded`.
    Superseded,
    /// Recorded `accepted` and the floor advanced: hand it to the platform.
    Accepted,
}

impl Ledger {
    fn position(&self, id: &str) -> Option<usize> {
        self.ring.iter().position(|e| e.id == id)
    }

    /// The recorded result for `id`, if the ring still holds it.
    #[cfg(test)]
    pub fn result_of(&self, id: &str) -> Option<ScreenLockResetResult> {
        self.position(id).map(|i| self.ring[i].result)
    }

    /// A re-delivery: the id is in the ring, or has left it and is still
    /// remembered. Without the second, an id the ring had dropped would be
    /// recorded afresh on every relay round for the rest of its two-day
    /// re-delivery, dropping the next one in turn.
    fn is_duplicate(&self, id: &str) -> bool {
        self.position(id).is_some() || self.forgotten.iter().any(|f| f == id)
    }

    /// Record `entry` as the newest; a full ring drops its earliest recorded
    /// entry, whose id joins `forgotten`.
    fn push(&mut self, entry: RingEntry) {
        self.ring.insert(0, entry);
        while self.ring.len() > RING_LEN {
            if let Some(dropped) = self.ring.pop() {
                self.forgotten.insert(0, dropped.id);
            }
        }
        self.forgotten.truncate(FORGOTTEN_LEN);
    }

    /// Decide and record an AUTHENTICATED reset (signature, signer, machine
    /// and pairing epoch already checked) at `now`, in the contract's order:
    /// re-delivery, then the window, then the floor.
    pub fn admit(&mut self, p: &ScreenLockResetPayload, now: u64) -> Admission {
        if self.is_duplicate(&p.id) {
            return Admission::Duplicate;
        }
        let (result, admission) =
            match charter_verify::check_screen_lock_reset_time(p, now, self.floor) {
                Ok(()) => (ScreenLockResetResult::Accepted, Admission::Accepted),
                Err(ScreenLockResetError::OutsideWindow) => {
                    (ScreenLockResetResult::RejectedTime, Admission::RejectedTime)
                }
                Err(_) => (ScreenLockResetResult::Superseded, Admission::Superseded),
            };
        if admission == Admission::Accepted {
            self.floor = p.issued_at;
        }
        self.push(RingEntry {
            id: p.id.clone(),
            result,
            at: now,
        });
        admission
    }

    /// Replace `id`'s `accepted` with its final `result`, recorded at `at`
    /// (it becomes the newest entry). `false`, changing nothing, when the
    /// ring holds no `accepted` entry for `id`: exactly one final outcome is
    /// ever recorded per id.
    pub fn settle(&mut self, id: &str, result: ScreenLockResetResult, at: u64) -> bool {
        let Some(i) = self.position(id) else {
            return false;
        };
        if self.ring[i].result != ScreenLockResetResult::Accepted {
            return false;
        }
        let mut e = self.ring.remove(i);
        e.result = result;
        e.at = at;
        self.ring.insert(0, e);
        true
    }

    /// The ring as STATUS carries it, newest first.
    pub fn outcomes(&self) -> Vec<ScreenLockResetOutcome> {
        self.ring
            .iter()
            .map(|e| ScreenLockResetOutcome {
                id: e.id.clone(),
                result: e.result.as_str().to_string(),
                at: e.at,
            })
            .collect()
    }
}

/// Why the stored ledger could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// The file exists but would not read (an EIO, a permissions change):
    /// try again later, and accept nothing meanwhile.
    Unreadable,
}

/// The ledger on disk: `<base>/screen-lock/ledger.json`.
pub struct LedgerStore {
    dir: PathBuf,
}

impl LedgerStore {
    pub fn new(base: &Path) -> LedgerStore {
        LedgerStore {
            dir: base.join("screen-lock"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("ledger.json")
    }

    /// The stored ledger. Absent is a fresh, empty one. A file that reads but
    /// will not parse is also treated as fresh (and replaced on the next
    /// write): a torn write cannot happen (each is a rename), so this is a
    /// file someone else damaged, and refusing every reset for ever over it
    /// would shut the guardian out of the ward's phone for good. The cost is
    /// that a reset from the last hour may apply once more. A file that will
    /// not READ is [`LoadError::Unreadable`].
    pub fn load(&self) -> Result<Ledger, LoadError> {
        match std::fs::read(self.path()) {
            Ok(bytes) => match serde_json::from_slice::<Ledger>(&bytes) {
                Ok(l) => Ok(l),
                Err(_) => {
                    eprintln!(
                        "charter: the screen-lock record would not parse; starting a new one"
                    );
                    Ok(Ledger::default())
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
            Err(_) => Err(LoadError::Unreadable),
        }
    }

    /// Write the ledger atomically (a temporary file renamed over it).
    /// Whether it landed.
    pub fn save(&self, l: &Ledger) -> bool {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let Ok(json) = serde_json::to_vec(l) else {
            return false;
        };
        let tmp = self.dir.join("ledger.json.tmp");
        let ok = std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, self.path()).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
        ok
    }
}

/// The one accepted reset waiting for Kotlin. Memory only; the PIN is wiped
/// when this drops.
pub struct Pending {
    pub id: String,
    pub new_pin: Zeroizing<String>,
    pub issued_at: u64,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("id", &self.id)
            .field("removes_screen_lock", &self.new_pin.is_empty())
            .field("issued_at", &self.issued_at)
            .finish_non_exhaustive()
    }
}

impl Pending {
    pub fn from_payload(p: &ScreenLockResetPayload) -> Pending {
        Pending {
            id: p.id.clone(),
            new_pin: Zeroizing::new(p.new_pin.clone()),
            issued_at: p.issued_at,
        }
    }

    /// `charterTakeScreenLockReset`'s body: `{"id":"…","newPin":"…","ts":N}`.
    /// The id is 32 hex characters and the PIN only digits (both validated
    /// on the way in), so neither needs escaping.
    pub fn to_json(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{{\"id\":\"{}\",\"newPin\":\"{}\",\"ts\":{}}}",
            self.id,
            self.new_pin.as_str(),
            self.issued_at
        ))
    }
}

/// The device AUDIT for one reset outcome: tags only, content empty, like
/// every device audit. Carries the reset's `id` and `at`, NEVER the PIN.
pub fn audit_tags(result: ScreenLockResetResult, id: &str, at: u64) -> Vec<Vec<String>> {
    // NB: the transport prepends the ["t","charter-device"] marker itself.
    vec![
        vec!["outcome".into(), result.as_str().into()],
        vec!["op".into(), AUDIT_OP.into()],
        vec!["id".into(), id.into()],
        vec!["at".into(), at.to_string()],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::PubKey;

    const NOW: u64 = 1_700_000_000;

    fn id(n: u64) -> String {
        format!("{n:032x}")
    }

    fn p(n: u64, issued_at: u64) -> ScreenLockResetPayload {
        ScreenLockResetPayload {
            v: 1,
            id: id(n),
            machine: PubKey::from_bytes([7; 32]),
            issued_at,
            new_pin: "123456".into(),
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("charter-slr-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn an_accepted_reset_advances_the_floor_and_a_redelivery_is_a_no_op() {
        let mut l = Ledger::default();
        assert_eq!(l.admit(&p(1, NOW), NOW), Admission::Accepted);
        assert_eq!(l.floor, NOW);
        assert_eq!(l.result_of(&id(1)), Some(ScreenLockResetResult::Accepted));
        let before = l.clone();
        // The same id, even hours later (the relays re-deliver for two days):
        // nothing changes, so an `applied` never turns into `rejected_time`.
        assert!(l.settle(&id(1), ScreenLockResetResult::Applied, NOW + 5));
        let settled = l.clone();
        assert_eq!(l.admit(&p(1, NOW), NOW + 7200), Admission::Duplicate);
        assert_eq!(l, settled);
        assert_ne!(before, settled);
    }

    #[test]
    fn stale_future_and_below_floor_each_record_one_outcome() {
        let mut l = Ledger::default();
        assert_eq!(l.admit(&p(1, NOW - 3601), NOW), Admission::RejectedTime);
        assert_eq!(l.admit(&p(2, NOW + 301), NOW), Admission::RejectedTime);
        assert_eq!(l.floor, 0, "a rejected reset moves no floor");
        assert_eq!(l.admit(&p(3, NOW), NOW), Admission::Accepted);
        assert_eq!(
            l.admit(&p(4, NOW), NOW),
            Admission::Superseded,
            "same second"
        );
        assert_eq!(l.admit(&p(5, NOW - 10), NOW), Admission::Superseded);
        assert_eq!(l.admit(&p(6, NOW + 1), NOW + 1), Admission::Accepted);
        let results: Vec<_> = l.outcomes().into_iter().map(|o| o.result).collect();
        assert_eq!(
            results,
            vec![
                "accepted",
                "superseded",
                "superseded",
                "accepted",
                "rejected_time",
                "rejected_time"
            ],
            "newest first"
        );
        // Each is recorded once: a second copy of any of them changes nothing.
        let sent = [NOW - 3601, NOW + 301, NOW, NOW, NOW - 10, NOW + 1];
        for (n, issued_at) in (1..=6).zip(sent) {
            assert_eq!(l.admit(&p(n, issued_at), NOW + 2), Admission::Duplicate);
        }
    }

    #[test]
    fn settle_replaces_only_accepted() {
        let mut l = Ledger::default();
        l.admit(&p(1, NOW), NOW);
        l.admit(&p(2, NOW - 4000), NOW);
        assert!(
            !l.settle(&id(2), ScreenLockResetResult::Applied, NOW),
            "not accepted"
        );
        assert!(
            !l.settle(&id(9), ScreenLockResetResult::Applied, NOW),
            "unknown id"
        );
        assert!(l.settle(&id(1), ScreenLockResetResult::NotReady, NOW + 3));
        assert!(
            !l.settle(&id(1), ScreenLockResetResult::Applied, NOW + 4),
            "once only"
        );
        assert_eq!(l.ring[0].id, id(1), "the settled entry is the newest");
        assert_eq!(l.ring[0].at, NOW + 3);
        assert_eq!(l.result_of(&id(1)), Some(ScreenLockResetResult::NotReady));
    }

    #[test]
    fn a_full_ring_forgets_in_recording_order_and_never_churns() {
        let mut l = Ledger::default();
        for n in 0..RING_LEN as u64 {
            assert_eq!(l.admit(&p(n, NOW + n), NOW + n), Admission::Accepted);
        }
        assert_eq!(l.ring.len(), RING_LEN);
        // A seventeenth drops id 0, the earliest recorded, into `forgotten`.
        let n = RING_LEN as u64;
        assert_eq!(l.admit(&p(n, NOW + n), NOW + n), Admission::Accepted);
        assert_eq!(l.ring.len(), RING_LEN);
        assert_eq!(l.result_of(&id(0)), None);
        assert_eq!(l.forgotten, vec![id(0)]);
        // Id 0 re-delivered on every later round: answered, so nothing moves.
        let before = l.clone();
        for round in 0..3 {
            assert_eq!(l.admit(&p(0, NOW), NOW + n + round), Admission::Duplicate);
        }
        assert_eq!(l, before);
        // `forgotten` is bounded too.
        for m in 0..(FORGOTTEN_LEN as u64 + 5) {
            l.admit(&p(1000 + m, NOW + 1000 + m), NOW + 1000 + m);
        }
        assert_eq!(l.ring.len(), RING_LEN);
        assert_eq!(l.forgotten.len(), FORGOTTEN_LEN);
    }

    /// Resets dated far ahead of the ward's clock are recorded
    /// `rejected_time` and must never crowd out a correctly dated one: the
    /// ring forgets in recording order, whatever the dates.
    #[test]
    fn a_ring_full_of_future_dated_rejections_still_accepts_a_good_reset() {
        let mut l = Ledger::default();
        for n in 0..RING_LEN as u64 {
            assert_eq!(
                l.admit(&p(n, NOW + 86_400 + n), NOW),
                Admission::RejectedTime
            );
        }
        assert_eq!(l.admit(&p(99, NOW), NOW), Admission::Accepted);
        assert_eq!(l.result_of(&id(99)), Some(ScreenLockResetResult::Accepted));
        assert!(l.settle(&id(99), ScreenLockResetResult::Applied, NOW + 1));
    }

    #[test]
    fn the_store_round_trips_and_treats_absent_and_damaged_as_fresh() {
        let dir = tmp("store");
        let s = LedgerStore::new(&dir);
        assert_eq!(s.load(), Ok(Ledger::default()), "absent");
        let mut l = Ledger::default();
        l.admit(&p(1, NOW), NOW);
        assert!(s.save(&l));
        assert_eq!(s.load(), Ok(l.clone()));
        let raw = std::fs::read_to_string(dir.join("screen-lock/ledger.json")).unwrap();
        assert!(!raw.contains("123456"), "no PIN on disk: {raw}");
        assert!(raw.contains("\"floor\":1700000000"), "{raw}");
        std::fs::write(dir.join("screen-lock/ledger.json"), "{not json").unwrap();
        assert_eq!(s.load(), Ok(Ledger::default()), "damaged");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_never_shows_its_pin_except_in_the_take_body() {
        let pending = Pending::from_payload(&p(1, NOW));
        let dbg = format!("{pending:?}");
        assert!(!dbg.contains("123456"), "{dbg}");
        assert_eq!(
            pending.to_json().as_str(),
            format!(
                "{{\"id\":\"{}\",\"newPin\":\"123456\",\"ts\":{NOW}}}",
                id(1)
            )
        );
        let v: serde_json::Value = serde_json::from_str(&pending.to_json()).unwrap();
        assert_eq!(v["ts"], NOW);
    }

    #[test]
    fn audit_tags_carry_id_and_at_and_never_a_pin() {
        let tags = audit_tags(ScreenLockResetResult::Applied, &id(1), NOW);
        assert_eq!(
            tags,
            vec![
                vec!["outcome".to_string(), "applied".to_string()],
                vec!["op".to_string(), "screenlock.reset".to_string()],
                vec!["id".to_string(), id(1)],
                vec!["at".to_string(), NOW.to_string()],
            ]
        );
    }
}
