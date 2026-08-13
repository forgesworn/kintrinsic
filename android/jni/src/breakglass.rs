//! The break-glass override (design memo
//! `docs/superpowers/specs/2026-07-24-lifeline-incall-and-break-glass.md`).
//!
//! The fire-alarm model: nothing stops the ward breaking the glass, but
//! breaking it is LOUD. Two durable pieces live here:
//!
//! 1. the active window (`{at, scope, duration}`) — survives restart, so a
//!    reboot mid-emergency can't slam the door;
//! 2. a pending-audit spool — the unlock NEVER waits on the wire, so offline
//!    the audit is journaled and emitted on the next successful poll.
//!
//! Deliberately NO rate limit and NO cooldown: a technical cap on an
//! emergency button rebuilds the cage. Overuse is a conversation, and the
//! transparency is what starts it.

use std::path::{Path, PathBuf};

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
        let mut all = self.pending_audits();
        all.push(tags);
        if let Ok(json) = serde_json::to_string(&all) {
            let _ = std::fs::write(self.spool_path(), json);
        }
    }

    pub fn pending_audits(&self) -> Vec<Vec<Vec<String>>> {
        std::fs::read_to_string(self.spool_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
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
