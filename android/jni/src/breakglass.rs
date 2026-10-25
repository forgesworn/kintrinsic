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
//!    is offered the wrap when it is built) sees one override, not one per
//!    poll round the relays were down.
//!
//! Deliberately NO rate limit and NO cooldown: a technical cap on an
//! emergency button rebuilds the cage. Overuse is a conversation, and the
//! transparency is what starts it.

use std::path::{Path, PathBuf};

use charter_primitives::{EventId, NostrEvent};

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
        all.push(SpooledAudit { tags, wrap: None });
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
                StoredAudit::Entry(e) => e,
                StoredAudit::Legacy(tags) => SpooledAudit { tags, wrap: None },
            })
            .collect()
    }

    fn write_entries(&self, all: &[SpooledAudit]) {
        if all.is_empty() {
            self.clear_audits();
            return;
        }
        if let Ok(json) = serde_json::to_string(all) {
            let _ = std::fs::write(self.spool_path(), json);
        }
    }

    /// Give every journaled audit that has none its wrap (`build`, which also
    /// offers it to the local link), store it with the entry, and return the
    /// wraps to publish, oldest first. An entry whose build fails keeps
    /// waiting and is tried again next round.
    pub fn wraps_to_publish(
        &self,
        mut build: impl FnMut(&[Vec<String>]) -> Option<NostrEvent>,
    ) -> Vec<NostrEvent> {
        let mut all = self.entries();
        let mut built = false;
        for e in all.iter_mut().filter(|e| e.wrap.is_none()) {
            e.wrap = build(&e.tags);
            built |= e.wrap.is_some();
        }
        if built {
            self.write_entries(&all);
        }
        all.into_iter().filter_map(|e| e.wrap).collect()
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

    /// Drain the spool (called once the emits have been handed to the relay).
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

    fn fake_wrap(n: u8) -> NostrEvent {
        NostrEvent {
            id: EventId::from_bytes([n; 32]),
            pubkey: charter_primitives::PubKey::from_bytes([2; 32]),
            created_at: 1,
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
            Some(fake_wrap(builds))
        };
        let first = bg.wraps_to_publish(&mut build);
        let again = BreakGlass::new(&base).wraps_to_publish(&mut build);
        assert_eq!(
            first, again,
            "the stored wrap is reused, across restarts too"
        );
        assert_eq!(first.len(), 1);
        // A second audit gets its own wrap; the first keeps its own.
        bg.queue_audit(audit_tags(SCOPE_CALLS, 300));
        let both = bg.wraps_to_publish(&mut build);
        assert_eq!(builds, 2);
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
        let wraps = bg.wraps_to_publish(|_| Some(fake_wrap(9)));
        assert_eq!(wraps, vec![fake_wrap(9)]);
        assert_eq!(bg.entries()[0].wrap, Some(fake_wrap(9)));
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
