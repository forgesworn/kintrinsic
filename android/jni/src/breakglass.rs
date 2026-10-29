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
//!    than two days from its own clock. With more than one guardian key
//!    (contract rule 9) an entry holds one wrap per recipient, and leaves the
//!    journal only once every one of them has reached a relay.
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

/// One journaled audit: its tags and, once built, the gift-wraps (one per
/// recipient, rule 9) that every retry re-publishes verbatim. A wrap leaves
/// `wraps` once a relay has taken it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpooledAudit {
    pub tags: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wraps: Vec<NostrEvent>,
    /// The single wrap an earlier build journaled; read into `wraps`.
    #[serde(default, skip_serializing)]
    wrap: Option<NostrEvent>,
}

impl SpooledAudit {
    fn new(tags: Vec<Vec<String>>) -> Self {
        SpooledAudit {
            tags,
            wraps: Vec::new(),
            wrap: None,
        }
    }

    /// An entry as read: an earlier build's single wrap joins `wraps`.
    fn normalised(mut self) -> Self {
        if let Some(w) = self.wrap.take() {
            if self.wraps.is_empty() {
                self.wraps.push(w);
            }
        }
        self
    }
}

/// A spool entry as stored: the current shape, or the tags-only list an
/// earlier build wrote (its wrap is built on the next round).
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredAudit {
    Entry(Box<SpooledAudit>),
    Legacy(Vec<Vec<String>>),
}

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
            let _ = std::fs::write(self.window_path(), json);
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
    pub fn queue_audit(&self, tags: Vec<Vec<String>>) {
        let mut all = self.entries();
        all.push(SpooledAudit::new(tags));
        self.write_entries(&all);
    }

    /// The journaled audits' tags, oldest first.
    #[cfg(test)]
    pub fn pending_audits(&self) -> Vec<Vec<Vec<String>>> {
        self.entries().into_iter().map(|e| e.tags).collect()
    }

    /// The journaled audits, oldest first.
    pub fn entries(&self) -> Vec<SpooledAudit> {
        std::fs::read_to_string(self.spool_path())
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<StoredAudit>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|e| match e {
                StoredAudit::Entry(e) => (*e).normalised(),
                StoredAudit::Legacy(tags) => SpooledAudit::new(tags),
            })
            .collect()
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
        let ok =
            std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, self.spool_path()).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
        ok
    }

    /// Give every journaled audit that has no wraps, or ones dated more than
    /// [`REBUILD_AFTER_SECS`] from `now`, new wraps (`build`: one per
    /// recipient, which must NOT be offered to the local link), store them
    /// with the entry, and only once that write has landed hand each new wrap
    /// to `offer` (the local link). Returns the wraps to publish, oldest
    /// first, never one outside the guardian's freshness window. An entry is
    /// never dropped for its age; one whose build yields nothing (no
    /// recipients, a failed build) keeps waiting and is tried again next
    /// round. A rebuild is for every current recipient, so one that already
    /// had its copy may get a second, a day later: one extra alert at most.
    ///
    /// If the journal cannot be written (a full disk) nothing is offered: an
    /// offer made for a wrap the journal does not hold would be followed by
    /// another, with a new id, every round.
    pub fn wraps_to_publish(
        &self,
        now: u64,
        mut build: impl FnMut(&[Vec<String>]) -> Vec<NostrEvent>,
        mut offer: impl FnMut(&NostrEvent),
    ) -> Vec<NostrEvent> {
        let mut all = self.entries();
        let mut fresh: Vec<NostrEvent> = Vec::new();
        for e in all.iter_mut() {
            let due = e.wraps.is_empty()
                || e.wraps
                    .iter()
                    .any(|w| now.abs_diff(w.created_at) > REBUILD_AFTER_SECS);
            if !due {
                continue;
            }
            let built = build(&e.tags);
            if !built.is_empty() {
                fresh.extend(built.iter().cloned());
                e.wraps = built;
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
            .flat_map(|e| e.wraps)
            .filter(|w| now.abs_diff(w.created_at) <= MAX_JITTER_SECS)
            .collect()
    }

    /// Forget the wraps a relay has taken, and drop each entry whose every
    /// wrap has gone.
    pub fn mark_sent(&self, sent: &[EventId]) {
        let mut all = self.entries();
        let mut changed = false;
        all.retain_mut(|e| {
            if e.wraps.is_empty() {
                return true;
            }
            let before = e.wraps.len();
            e.wraps.retain(|w| !sent.contains(&w.id));
            changed |= e.wraps.len() != before;
            !e.wraps.is_empty()
        });
        if changed {
            self.write_entries(&all);
        }
    }

    /// Drain the spool.
    #[cfg(test)]
    pub fn clear_audits(&self) {
        let _ = std::fs::remove_file(self.spool_path());
    }
}

/// The contract's break-glass audit tags (`spec/contract.md` §Break-glass
/// override event): outcome=override, op=unlock.breakglass, scope, duration.
/// Content is always empty — numbers and enums only, never PII.
pub fn audit_tags(scope: &str, duration_secs: u64) -> Vec<Vec<String>> {
    // NB: the transport prepends the ["t","charter-device"] marker itself.
    vec![
        vec!["outcome".into(), "override".into()],
        vec!["op".into(), "unlock.breakglass".into()],
        vec!["scope".into(), scope.into()],
        vec!["durationSecs".into(), duration_secs.to_string()],
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
        bg.queue_audit(audit_tags(SCOPE_FULL, 600));
        bg.queue_audit(audit_tags(SCOPE_CALLS, 300));
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
        bg.queue_audit(audit_tags(SCOPE_FULL, 600));
        let mut builds = 0u8;
        let mut build = |_: &[Vec<String>]| {
            builds += 1;
            vec![fake_wrap(builds)]
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
        bg.queue_audit(audit_tags(SCOPE_CALLS, 300));
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
        assert_eq!(bg.pending_audits(), vec![audit_tags(SCOPE_CALLS, 300)]);
        bg.mark_sent(&[both[1].id]);
        assert!(bg.pending_audits().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_spool_written_by_an_earlier_build_still_loads() {
        let base = tmp("legacy");
        let bg = BreakGlass::new(&base);
        let legacy = vec![audit_tags(SCOPE_FULL, 600)];
        std::fs::write(bg.spool_path(), serde_json::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(bg.pending_audits(), legacy);
        let wraps = bg.wraps_to_publish(T0, |_| vec![fake_wrap(9)], |_| {});
        assert_eq!(wraps, vec![fake_wrap(9)]);
        assert_eq!(bg.entries()[0].wraps, vec![fake_wrap(9)]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_journal_holding_one_wrap_from_an_earlier_build_keeps_it() {
        let base = tmp("onewrap");
        let bg = BreakGlass::new(&base);
        let old = serde_json::json!([{"tags": audit_tags(SCOPE_FULL, 600), "wrap": fake_wrap(3)}]);
        std::fs::write(bg.spool_path(), old.to_string()).unwrap();
        let wraps = bg.wraps_to_publish(T0, |_| panic!("not rebuilt"), |_| {});
        assert_eq!(wraps, vec![fake_wrap(3)]);
        bg.mark_sent(&[fake_wrap(3).id]);
        assert!(bg.entries().is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Rule 9: one wrap per recipient; the audit stays journaled until every
    /// one of them has reached a relay.
    #[test]
    fn an_audit_for_two_recipients_leaves_only_when_both_are_sent() {
        let base = tmp("two");
        let bg = BreakGlass::new(&base);
        bg.queue_audit(audit_tags(SCOPE_FULL, 600));
        let mut offered = 0;
        let wraps = bg.wraps_to_publish(T0, |_| vec![fake_wrap(1), fake_wrap(2)], |_| offered += 1);
        assert_eq!(wraps.len(), 2);
        assert_eq!(offered, 2, "each recipient's wrap offered once");
        bg.mark_sent(&[wraps[0].id]);
        assert_eq!(bg.entries()[0].wraps, vec![fake_wrap(2)]);
        let again = bg.wraps_to_publish(T0 + 60, |_| panic!("not rebuilt"), |_| {});
        assert_eq!(again, vec![fake_wrap(2)], "only the unsent one again");
        bg.mark_sent(&[wraps[1].id]);
        assert!(bg.entries().is_empty());
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
        bg.queue_audit(audit_tags(SCOPE_FULL, 600));
        let mut n = 0u8;
        let mut offered = Vec::new();
        let mut round = |now: u64, offered: &mut Vec<NostrEvent>| {
            bg.wraps_to_publish(
                now,
                |_| {
                    n += 1;
                    vec![fake_wrap_at(n, now)]
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
        assert_eq!(bg.entries()[0].wraps[0].id, rebuilt[0].id);

        // A wrap already outside the window (a clock that jumped) whose
        // rebuild fails is kept, but never handed out to publish.
        let far = T0 + 10 * REBUILD_AFTER_SECS;
        let none = bg.wraps_to_publish(far, |_| Vec::new(), |_| panic!("nothing new to offer"));
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
        bg.queue_audit(audit_tags(SCOPE_FULL, 600));
        // The temporary file's name is taken by a directory: the write fails.
        std::fs::create_dir_all(bg.dir.join("pending-audits.json.tmp")).unwrap();
        let mut offers = 0;
        let wraps = bg.wraps_to_publish(T0, |_| vec![fake_wrap(4)], |_| offers += 1);
        assert_eq!(offers, 0, "not offered while unjournaled");
        assert_eq!(wraps.len(), 1, "the relays still get it");
        assert!(bg.entries()[0].wraps.is_empty(), "the journal is unchanged");
        // The disk recovers: journaled, then offered, once.
        std::fs::remove_dir_all(bg.dir.join("pending-audits.json.tmp")).unwrap();
        let wraps = bg.wraps_to_publish(T0 + 15, |_| vec![fake_wrap(5)], |_| offers += 1);
        assert_eq!(offers, 1);
        assert_eq!(bg.entries()[0].wraps, vec![wraps[0].clone()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn audit_tags_carry_no_pii() {
        let tags = audit_tags(SCOPE_FULL, 600);
        let flat = format!("{tags:?}");
        // Only routing/classification vocabulary — no names, numbers, reasons.
        for key in ["outcome", "op", "scope", "durationSecs"] {
            assert!(flat.contains(key));
        }
        assert_eq!(tags.len(), 4, "no extra fields creep in");
    }
}
