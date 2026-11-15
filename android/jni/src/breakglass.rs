//! The break-glass override (internal design spec
//! 2026-07-24-lifeline-incall-and-break-glass).
//!
//! The fire-alarm model: nothing stops the ward breaking the glass, but
//! breaking it is LOUD. Two durable pieces live here:
//!
//! 1. the active window (`{at, scope, duration}`) — survives restart, so a
//!    reboot mid-emergency can't slam the door;
//! 2. a pending-audit spool — the unlock NEVER waits on the wire, so offline
//!    the audit is journaled and emitted on the next successful poll. Each
//!    entry's gift-wrap is built ONCE and kept with it, so every retry is the
//!    identical event: the guardian (on the relays or the local link, which
//!    is offered the wrap once it is journaled) sees one override, not one per
//!    poll round the relays were down. A wrap that has waited a day is built
//!    afresh (once a day), because the guardian refuses a wrap dated more
//!    than two days from its own clock.
//!
//! Deliberately NO rate limit and NO cooldown: a technical cap on an
//! emergency button rebuilds the cage. Overuse is a conversation, and the
//! transparency is what starts it.

use std::path::{Path, PathBuf};

use charter_primitives::{EventId, NostrEvent};
use charter_transport::nip59::MAX_JITTER_SECS;

/// A journaled wrap dated this far from now is rebuilt before it is published
/// again. Every guardian path (the carrier, the PWA's relay read and LAN
/// drain) refuses a wrap more than [`MAX_JITTER_SECS`] (two days) from its own
/// clock, so rebuilding at one day keeps every copy well inside that however
/// long the ward stays offline. Once a day is one extra alert at most.
pub const REBUILD_AFTER_SECS: u64 = 24 * 3600;

/// How much the override opens.
pub const SCOPE_CALLS: &str = "calls";
pub const SCOPE_FULL: &str = "full";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Window {
    /// Unix seconds the ward broke the glass.
    pub at: u64,
    /// `calls` | `full`.
    pub scope: String,
    pub duration_secs: u64,
}

impl Window {
    /// Still open at `now`? (A clock that jumps backwards keeps it open —
    /// fail-OPEN is correct here and only here: this is the safety valve.)
    pub fn active(&self, now: u64) -> bool {
        now < self.at.saturating_add(self.duration_secs)
    }
    /// How much of the safety valve is left. Unused today — the shade renders
    /// its own countdown — but kept beside `active` so the two cannot drift.
    #[allow(dead_code)]
    pub fn secs_left(&self, now: u64) -> u64 {
        self.at
            .saturating_add(self.duration_secs)
            .saturating_sub(now)
    }
}

/// One journaled audit: its tags and, once built, the gift-wrap that every
/// retry re-publishes verbatim.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpooledAudit {
    pub tags: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<NostrEvent>,
}

/// A spool entry as stored: the current shape, or the tags-only list an
/// earlier build wrote (its wrap is built on the next round).
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredAudit {
    Entry(SpooledAudit),
    Legacy(Vec<Vec<String>>),
}

/// Write `bytes` to `tmp`, flush them to disk, rename `tmp` over `dest`, then
/// flush the directory entry, so a press that has been journaled survives a
/// power cut and never leaves half a file. Whether it all landed.
fn write_durably(tmp: &Path, dest: &Path, bytes: &[u8]) -> bool {
    use std::io::Write;
    let wrote = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })()
    .is_ok();
    if !wrote || std::fs::rename(tmp, dest).is_err() {
        return false;
    }
    // The rename is only durable once the directory has been synced. Not every
    // filesystem lets a directory be opened or synced; the data itself is
    // already on disk, so a refusal here is not a failed write, but it is
    // logged: on such a filesystem a power cut can undo the rename.
    if let Some(dir) = dest.parent() {
        match std::fs::File::open(dir).and_then(|d| d.sync_all()) {
            Ok(()) => {}
            Err(e) => eprintln!(
                "charter: break-glass directory sync refused ({:?}); the rename may not survive a power cut",
                e.kind()
            ),
        }
    }
    true
}

/// How many moved-aside unreadable spools are kept.
const KEEP_BAD_SPOOLS: usize = 3;

/// Durable store for the window + the audit spool.
pub struct BreakGlass {
    dir: PathBuf,
}

impl BreakGlass {
    pub fn new(base: &Path) -> BreakGlass {
        let dir = base.join("breakglass");
        let _ = std::fs::create_dir_all(&dir);
        BreakGlass { dir }
    }

    fn window_path(&self) -> PathBuf {
        self.dir.join("window.json")
    }
    fn spool_path(&self) -> PathBuf {
        self.dir.join("pending-audits.json")
    }

    /// Record a fresh override window (replaces any previous one).
    pub fn open(&self, w: &Window) {
        if let Ok(json) = serde_json::to_string(w) {
            let tmp = self.dir.join("window.json.tmp");
            if !write_durably(&tmp, &self.window_path(), json.as_bytes()) {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    /// The stored window, if any (expired ones are still returned — callers
    /// ask `active()`; keeping it lets the shade say "that's finished now").
    pub fn window(&self) -> Option<Window> {
        let raw = std::fs::read_to_string(self.window_path()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Queue an audit for the guardian. Durable: the ward's unlock happens
    /// immediately regardless of the network, and this rides out on the next
    /// successful poll.
    pub fn queue_audit(&self, tags: Vec<Vec<String>>) -> bool {
        let mut all = self.entries();
        all.push(SpooledAudit { tags, wrap: None });
        self.write_entries(&all)
    }

    /// The journaled audits' tags, oldest first.
    #[cfg(test)]
    pub fn pending_audits(&self) -> Vec<Vec<Vec<String>>> {
        self.entries().into_iter().map(|e| e.tags).collect()
    }

    /// The journaled audits, oldest first.
    ///
    /// A missing spool is an empty one. A spool that cannot be read or parsed
    /// is NOT: it is moved aside (see [`BreakGlass::set_spool_aside`]) before
    /// this returns an empty list, so the next write can never overwrite
    /// pending break-glass, screen-lock or emergency entries that might still
    /// be recovered.
    pub fn entries(&self) -> Vec<SpooledAudit> {
        let raw = match std::fs::read_to_string(self.spool_path()) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                eprintln!("charter: break-glass spool unreadable ({:?})", e.kind());
                self.set_spool_aside();
                return Vec::new();
            }
        };
        match serde_json::from_str::<Vec<StoredAudit>>(&raw) {
            Ok(list) => list
                .into_iter()
                .map(|e| match e {
                    StoredAudit::Entry(e) => e,
                    StoredAudit::Legacy(tags) => SpooledAudit { tags, wrap: None },
                })
                .collect(),
            Err(_) => {
                eprintln!("charter: break-glass spool does not parse");
                self.set_spool_aside();
                Vec::new()
            }
        }
    }

    /// Rename an unusable spool to `pending-audits.json.bad-<unix>` (never
    /// overwriting an earlier one), keeping the newest [`KEEP_BAD_SPOOLS`] of
    /// them, so the data stays on the phone for recovery and the next write
    /// starts a fresh file.
    fn set_spool_aside(&self) {
        const PREFIX: &str = "pending-audits.json.bad-";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let existing = |dir: &Path| -> Vec<(u64, PathBuf)> {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let n = name.strip_prefix(PREFIX)?.parse::<u64>().ok()?;
                    Some((n, e.path()))
                })
                .collect()
        };
        // Always newer than every one already kept, even if the clock stood
        // still or stepped back, so the newest is never the one pruned.
        let stamp = existing(&self.dir)
            .iter()
            .map(|(n, _)| n.saturating_add(1))
            .fold(now, u64::max);
        let aside = self.dir.join(format!("{PREFIX}{stamp}"));
        if let Err(e) = std::fs::rename(self.spool_path(), &aside) {
            eprintln!(
                "charter: break-glass spool could not be set aside ({:?})",
                e.kind()
            );
            return;
        }
        eprintln!("charter: break-glass spool set aside as {PREFIX}{stamp}");
        let mut bad = existing(&self.dir);
        bad.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        for (_, path) in bad.into_iter().skip(KEEP_BAD_SPOOLS) {
            let _ = std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path));
        }
    }

    /// Write the spool (atomically: a temporary file renamed over it, so a
    /// crash or a full disk never leaves half a journal). Whether it landed.
    fn write_entries(&self, all: &[SpooledAudit]) -> bool {
        if all.is_empty() {
            return match std::fs::remove_file(self.spool_path()) {
                Ok(()) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
        }
        let Ok(json) = serde_json::to_string(all) else {
            return false;
        };
        let tmp = self.dir.join("pending-audits.json.tmp");
        let ok = write_durably(&tmp, &self.spool_path(), json.as_bytes());
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
        ok
    }

    /// Give every journaled audit that has no wrap, or one dated more than
    /// [`REBUILD_AFTER_SECS`] from `now`, a new wrap (`build`, which must NOT
    /// offer it to the local link), store it with the entry, and only once
    /// that write has landed hand each new wrap to `offer` (the local link).
    /// Returns the wraps to publish, oldest first, never one outside the
    /// guardian's freshness window. An entry is never dropped for its age; one
    /// whose build fails keeps waiting and is tried again next round.
    ///
    /// If the journal cannot be written (a full disk) nothing is offered: an
    /// offer made for a wrap the journal does not hold would be followed by
    /// another, with a new id, every round.
    pub fn wraps_to_publish(
        &self,
        now: u64,
        mut build: impl FnMut(&[Vec<String>]) -> Option<NostrEvent>,
        mut offer: impl FnMut(&NostrEvent),
    ) -> Vec<NostrEvent> {
        let mut all = self.entries();
        let mut fresh: Vec<NostrEvent> = Vec::new();
        for e in all.iter_mut() {
            let due = e
                .wrap
                .as_ref()
                .is_none_or(|w| now.abs_diff(w.created_at) > REBUILD_AFTER_SECS);
            if !due {
                continue;
            }
            if let Some(w) = build(&e.tags) {
                fresh.push(w.clone());
                e.wrap = Some(w);
            }
        }
        if !fresh.is_empty() {
            if self.write_entries(&all) {
                for w in &fresh {
                    offer(w);
                }
            } else {
                eprintln!("charter: break-glass journal not written; the local link is not offered its audit this round");
            }
        }
        all.into_iter()
            .filter_map(|e| e.wrap)
            .filter(|w| now.abs_diff(w.created_at) <= MAX_JITTER_SECS)
            .collect()
    }

    /// Drop the entries whose wrap a relay has taken.
    pub fn mark_sent(&self, sent: &[EventId]) {
        let mut all = self.entries();
        let before = all.len();
        all.retain(|e| !e.wrap.as_ref().is_some_and(|w| sent.contains(&w.id)));
        if all.len() != before {
            self.write_entries(&all);
        }
    }

    /// Drain the spool (a guardian's release, once its held audits have had
    /// their one publish). Whether it is gone.
    pub fn clear_audits(&self) -> bool {
        self.write_entries(&[])
    }
}

/// The contract's break-glass audit tags (`spec/contract.md` §Break-glass
/// override event): outcome=override, op=unlock.breakglass, scope, duration,
/// and `at`, the moment the ward pressed it. Content is always empty —
/// numbers and enums only, never PII.
///
/// `at` is carried as a tag because the audit's gift-wrap is stamped when it
/// is BUILT: at the next poll after the press, even if the phone is offline
/// (the wrap then waits, with that old date, until a relay takes it), and again
/// if it is rebuilt after a day. Neither is when the glass was broken. A tag is
/// part of the journaled entry, so it is stable across every rebuild and the
/// guardian can fold the copies into one line at the right time.
pub fn audit_tags(scope: &str, duration_secs: u64, at: u64) -> Vec<Vec<String>> {
    // NB: the transport prepends the ["t","charter-device"] marker itself.
    vec![
        vec!["outcome".into(), "override".into()],
        vec!["op".into(), "unlock.breakglass".into()],
        vec!["scope".into(), scope.into()],
        vec!["durationSecs".into(), duration_secs.to_string()],
        vec!["at".into(), at.to_string()],
    ]
}

/// The op of the emergency-call audit (`spec/contract.md` §Emergency call
/// event).
pub const OP_EMERGENCY_CALL: &str = "call.emergency";

/// The contract's emergency-call audit tags: outcome=override,
/// op=call.emergency and `at`, the moment the ward's hold and cancel window
/// completed. Nothing else: never a number, a duration, a reason or anything
/// personal, and the content is empty. It means the control completed its
/// hold and cancel window, and the ward app asked the phone to open its
/// emergency dialler. It is not proof that the dialler appeared or that a call
/// was placed.
///
/// Like [`audit_tags`], `at` is a tag because the wrap is stamped when it is
/// built, not when the button was pressed.
pub fn emergency_call_audit_tags(at: u64) -> Vec<Vec<String>> {
    // NB: the transport prepends the ["t","charter-device"] marker itself.
    vec![
        vec!["outcome".into(), "override".into()],
        vec!["op".into(), OP_EMERGENCY_CALL.into()],
        vec!["at".into(), at.to_string()],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("charter-bg-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn window_persists_and_expires() {
        let base = tmp("win");
        let bg = BreakGlass::new(&base);
        assert!(bg.window().is_none());
        bg.open(&Window {
            at: 1_000,
            scope: SCOPE_FULL.into(),
            duration_secs: 600,
        });
        // A "restart": a fresh handle over the same dir still sees it.
        let bg2 = BreakGlass::new(&base);
        let w = bg2.window().expect("survives restart");
        assert!(w.active(1_500), "still inside the window");
        assert_eq!(w.secs_left(1_500), 100);
        assert!(!w.active(1_600), "expired at the boundary");
        assert_eq!(w.secs_left(1_600), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn audits_spool_durably_and_drain() {
        let base = tmp("spool");
        let bg = BreakGlass::new(&base);
        assert!(bg.pending_audits().is_empty());
        bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0));
        bg.queue_audit(audit_tags(SCOPE_CALLS, 300, T0));
        let reopened = BreakGlass::new(&base);
        let pending = reopened.pending_audits();
        assert_eq!(pending.len(), 2, "offline audits survive a restart");
        assert!(pending[0].contains(&vec!["outcome".into(), "override".into()]));
        assert!(pending[0].contains(&vec!["op".into(), "unlock.breakglass".into()]));
        assert!(pending[1].contains(&vec!["scope".into(), "calls".into()]));
        assert!(pending[1].contains(&vec!["durationSecs".into(), "300".into()]));
        reopened.clear_audits();
        assert!(BreakGlass::new(&base).pending_audits().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A fixed "now" for the spool tests.
    const T0: u64 = 1_782_752_400;

    fn fake_wrap(n: u8) -> NostrEvent {
        fake_wrap_at(n, T0)
    }

    fn fake_wrap_at(n: u8, created_at: u64) -> NostrEvent {
        NostrEvent {
            id: EventId::from_bytes([n; 32]),
            pubkey: charter_primitives::PubKey::from_bytes([2; 32]),
            created_at,
            kind: charter_primitives::kinds::GIFT_WRAP,
            tags: vec![],
            content: String::new(),
            sig: charter_primitives::Sig::from_bytes([0; 64]),
        }
    }

    #[test]
    fn each_audit_is_built_once_and_dropped_only_when_sent() {
        let base = tmp("once");
        let bg = BreakGlass::new(&base);
        bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0));
        let mut builds = 0u8;
        let mut build = |_: &[Vec<String>]| {
            builds += 1;
            Some(fake_wrap(builds))
        };
        let mut offered = Vec::new();
        let first = bg.wraps_to_publish(T0, &mut build, |w| offered.push(w.id));
        let again =
            BreakGlass::new(&base).wraps_to_publish(T0 + 60, &mut build, |w| offered.push(w.id));
        assert_eq!(
            first, again,
            "the stored wrap is reused, across restarts too"
        );
        assert_eq!(first.len(), 1);
        // A second audit gets its own wrap; the first keeps its own.
        bg.queue_audit(audit_tags(SCOPE_CALLS, 300, T0));
        let both = bg.wraps_to_publish(T0 + 120, &mut build, |w| offered.push(w.id));
        assert_eq!(builds, 2);
        assert_eq!(
            offered,
            vec![first[0].id, both[1].id],
            "each wrap offered once"
        );
        assert_eq!(both[0], first[0]);
        assert_eq!(both.len(), 2);
        // Only what was sent leaves the spool.
        bg.mark_sent(&[both[0].id]);
        assert_eq!(bg.pending_audits(), vec![audit_tags(SCOPE_CALLS, 300, T0)]);
        bg.mark_sent(&[both[1].id]);
        assert!(bg.pending_audits().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_spool_written_by_an_earlier_build_still_loads() {
        let base = tmp("legacy");
        let bg = BreakGlass::new(&base);
        // Hand-written as an earlier build stored it: the tags it knew then,
        // with no `at`, so this proves old entries (not just current ones) load.
        let legacy: Vec<Vec<Vec<String>>> = vec![vec![
            vec!["outcome".into(), "override".into()],
            vec!["op".into(), "unlock.breakglass".into()],
            vec!["scope".into(), SCOPE_FULL.into()],
            vec!["durationSecs".into(), "600".into()],
        ]];
        assert!(legacy[0].iter().all(|t| t[0] != "at"));
        std::fs::write(bg.spool_path(), serde_json::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(bg.pending_audits(), legacy);
        let wraps = bg.wraps_to_publish(T0, |_| Some(fake_wrap(9)), |_| {});
        assert_eq!(wraps, vec![fake_wrap(9)]);
        assert_eq!(bg.entries()[0].wrap, Some(fake_wrap(9)));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// M2: a journaled wrap that has waited more than a day is built afresh
    /// (one new wrap, one new link offer) rather than published with a date
    /// the guardian refuses; it is never dropped for its age, and no copy
    /// outside the guardian's two-day window is ever handed out.
    #[test]
    fn a_wrap_waiting_over_a_day_is_rebuilt_once_and_never_sent_stale() {
        let base = tmp("stale");
        let bg = BreakGlass::new(&base);
        bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0));
        let mut n = 0u8;
        let mut offered = Vec::new();
        let mut round = |now: u64, offered: &mut Vec<NostrEvent>| {
            bg.wraps_to_publish(
                now,
                |_| {
                    n += 1;
                    Some(fake_wrap_at(n, now))
                },
                |w| offered.push(w.clone()),
            )
        };
        let first = round(T0, &mut offered);
        assert_eq!(first.len(), 1);
        // Still offline a day later (but not more): the same wrap.
        let later = round(T0 + REBUILD_AFTER_SECS, &mut offered);
        assert_eq!(later, first);
        assert_eq!(offered.len(), 1);
        // Past a day: one rebuild, one new offer, and the new wrap is current.
        let rebuilt = round(T0 + REBUILD_AFTER_SECS + 15, &mut offered);
        assert_eq!(rebuilt.len(), 1, "never dropped for its age");
        assert_ne!(rebuilt[0].id, first[0].id);
        assert_eq!(rebuilt[0].created_at, T0 + REBUILD_AFTER_SECS + 15);
        assert_eq!(offered.len(), 2);
        assert_eq!(offered[1].id, rebuilt[0].id);
        // The next rounds reuse it: rebuilt once, not every round.
        let again = round(T0 + REBUILD_AFTER_SECS + 30, &mut offered);
        assert_eq!(again, rebuilt);
        assert_eq!(offered.len(), 2);
        assert_eq!(bg.entries()[0].wrap.as_ref().unwrap().id, rebuilt[0].id);

        // A wrap already outside the window (a clock that jumped) whose
        // rebuild fails is kept, but never handed out to publish.
        let far = T0 + 10 * REBUILD_AFTER_SECS;
        let none = bg.wraps_to_publish(far, |_| None, |_| panic!("nothing new to offer"));
        assert!(
            none.is_empty(),
            "a wrap the guardian would refuse is not sent"
        );
        assert_eq!(bg.entries().len(), 1, "still journaled");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L1: a wrap the journal could not hold (a full disk) is not offered to
    /// the local link, so a failing disk cannot put a new offer, with a new
    /// id, in the link outbox every round.
    #[test]
    fn a_wrap_that_could_not_be_journaled_is_not_offered() {
        let base = tmp("nodisk");
        let bg = BreakGlass::new(&base);
        bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0));
        // The temporary file's name is taken by a directory: the write fails.
        std::fs::create_dir_all(bg.dir.join("pending-audits.json.tmp")).unwrap();
        let mut offers = 0;
        let wraps = bg.wraps_to_publish(T0, |_| Some(fake_wrap(4)), |_| offers += 1);
        assert_eq!(offers, 0, "not offered while unjournaled");
        assert_eq!(wraps.len(), 1, "the relays still get it");
        assert_eq!(bg.entries()[0].wrap, None, "the journal is unchanged");
        // The disk recovers: journaled, then offered, once.
        std::fs::remove_dir_all(bg.dir.join("pending-audits.json.tmp")).unwrap();
        let wraps = bg.wraps_to_publish(T0 + 15, |_| Some(fake_wrap(5)), |_| offers += 1);
        assert_eq!(offers, 1);
        assert_eq!(bg.entries()[0].wrap, Some(wraps[0].clone()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The press time rides in the tags, so every rebuilt wrap of the same
    /// audit tells the guardian the same moment.
    #[test]
    fn audit_tags_carry_the_press_time() {
        let tags = audit_tags(SCOPE_CALLS, 300, 1_782_752_461);
        assert!(tags.contains(&vec!["at".into(), "1782752461".into()]));
        assert_eq!(
            tags.iter().filter(|t| t[0] == "at").count(),
            1,
            "exactly one at tag"
        );
    }

    /// The emergency-call audit is exactly three tags: the outcome that makes
    /// the carrier alert, the op, and the press time. Never a number.
    #[test]
    fn emergency_call_tags_are_exact_and_carry_nothing_else() {
        let tags = emergency_call_audit_tags(1_782_752_461);
        assert_eq!(
            tags,
            vec![
                vec!["outcome".to_string(), "override".to_string()],
                vec!["op".to_string(), "call.emergency".to_string()],
                vec!["at".to_string(), "1782752461".to_string()],
            ]
        );
    }

    /// The durable write path leaves exactly the file the plain write did:
    /// the spool and the window hold the same JSON, no temporary file is left
    /// behind, and a second write replaces the first.
    #[test]
    fn the_durable_write_produces_the_same_files() {
        let base = tmp("durable");
        let bg = BreakGlass::new(&base);
        let w = Window {
            at: 1_000,
            scope: SCOPE_FULL.into(),
            duration_secs: 600,
        };
        bg.open(&w);
        assert_eq!(
            std::fs::read_to_string(bg.window_path()).unwrap(),
            serde_json::to_string(&w).unwrap()
        );
        assert!(bg.queue_audit(emergency_call_audit_tags(T0)));
        assert!(bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0)));
        let expected = serde_json::to_string(&bg.entries()).unwrap();
        assert_eq!(
            std::fs::read_to_string(bg.spool_path()).unwrap(),
            expected,
            "the spool is the entries, verbatim"
        );
        assert_eq!(bg.entries().len(), 2);
        let mut names: Vec<String> = std::fs::read_dir(&bg.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["pending-audits.json", "window.json"]);
        bg.open(&Window { at: 2_000, ..w });
        assert_eq!(bg.window().unwrap().at, 2_000);
        let _ = std::fs::remove_dir_all(&base);
    }

    fn bad_files(bg: &BreakGlass) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&bg.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("pending-audits.json.bad-"))
            .collect();
        v.sort();
        v
    }

    /// L3: no spool at all is an empty spool and leaves nothing behind.
    #[test]
    fn a_missing_spool_is_empty_and_sets_nothing_aside() {
        let base = tmp("nospool");
        let bg = BreakGlass::new(&base);
        assert!(bg.entries().is_empty());
        assert!(bad_files(&bg).is_empty());
        assert!(bg.queue_audit(emergency_call_audit_tags(T0)));
        assert_eq!(bg.entries().len(), 1);
        assert!(bad_files(&bg).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L3: a spool that does not parse is moved aside, whole, before the next
    /// press writes a fresh one; the damaged bytes are never overwritten.
    #[test]
    fn a_corrupt_spool_is_set_aside_not_overwritten() {
        let base = tmp("corrupt");
        let bg = BreakGlass::new(&base);
        let damaged = b"[[[\"outcome\",\"override\"],[\"op\",\"unlock.bre";
        std::fs::write(bg.spool_path(), damaged).unwrap();
        assert!(bg.queue_audit(emergency_call_audit_tags(T0)));
        let kept = bad_files(&bg);
        assert_eq!(kept.len(), 1, "the damaged spool is kept");
        assert_eq!(std::fs::read(bg.dir.join(&kept[0])).unwrap(), damaged);
        // The new spool holds only the new press and reads cleanly.
        let now = bg.entries();
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].tags, emergency_call_audit_tags(T0));
        assert_eq!(bad_files(&bg).len(), 1, "a good spool sets nothing aside");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L3: an empty file is damage too (a crash between create and write on a
    /// filesystem without the rename), not "no entries".
    #[test]
    fn an_empty_spool_file_is_set_aside() {
        let base = tmp("emptyfile");
        let bg = BreakGlass::new(&base);
        std::fs::write(bg.spool_path(), b"").unwrap();
        assert!(bg.entries().is_empty());
        assert_eq!(bad_files(&bg).len(), 1);
        assert!(!bg.spool_path().exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L3: a read error that is not "not found" (here the spool path is a
    /// directory, so the read fails) is also set aside, and the next write
    /// succeeds.
    #[test]
    fn an_unreadable_spool_is_set_aside_and_the_next_press_still_journals() {
        let base = tmp("unreadable");
        let bg = BreakGlass::new(&base);
        std::fs::create_dir_all(bg.spool_path()).unwrap();
        assert!(bg.entries().is_empty());
        assert_eq!(bad_files(&bg).len(), 1);
        assert!(bg.queue_audit(emergency_call_audit_tags(T0)));
        assert_eq!(bg.entries().len(), 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L3: the newest three moved-aside spools are kept and older ones go; two
    /// in the same second do not replace each other.
    #[test]
    fn only_the_three_newest_damaged_spools_are_kept() {
        let base = tmp("keep3");
        let bg = BreakGlass::new(&base);
        for i in 0..5u8 {
            std::fs::write(bg.spool_path(), format!("bad{i}")).unwrap();
            assert!(bg.entries().is_empty());
        }
        let kept = bad_files(&bg);
        assert_eq!(kept.len(), 3);
        let mut contents: Vec<String> = kept
            .iter()
            .map(|n| std::fs::read_to_string(bg.dir.join(n)).unwrap())
            .collect();
        contents.sort();
        assert_eq!(contents, ["bad2", "bad3", "bad4"], "the newest three");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// L4: clearing the spool for a release leaves nothing behind and is not
    /// an error when there is nothing to clear.
    #[test]
    fn clearing_the_spool_empties_it() {
        let base = tmp("clear");
        let bg = BreakGlass::new(&base);
        assert!(bg.clear_audits());
        bg.queue_audit(emergency_call_audit_tags(T0));
        bg.queue_audit(audit_tags(SCOPE_FULL, 600, T0));
        assert_eq!(bg.entries().len(), 2);
        assert!(bg.clear_audits());
        assert!(bg.entries().is_empty());
        assert!(!bg.spool_path().exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn audit_tags_carry_no_pii() {
        let tags = audit_tags(SCOPE_FULL, 600, T0);
        let flat = format!("{tags:?}");
        // Only routing/classification vocabulary — no names, numbers, reasons.
        for key in ["outcome", "op", "scope", "durationSecs", "at"] {
            assert!(flat.contains(key));
        }
        assert_eq!(tags.len(), 5, "no extra fields creep in");
    }
}
