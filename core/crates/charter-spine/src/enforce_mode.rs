//! Staged enforcement mode + the observe-mode log formatter. Pure logic, gated
//! behind neither `mock` nor `real`, so the standard (mock) test gate exercises
//! it. `runtime.rs` (real-only) reads [`EnforceMode::from_env`] and calls
//! [`observe_lines`] when the loop runs in `observe`.

use charter_schedule::EnforcerEffect;

use crate::child_policy::PolicySource;
use crate::enforcer_runtime::lock_message;
use crate::local_limits::user_for_uid;
use crate::multi_child::ChildDecision;

/// How much of the enforcement decision the loop actually applies. Any
/// unrecognised/absent `CHARTER_ENFORCE` value resolves to full `Enforce`, so a
/// production install is never accidentally soft. `observe` and `freeze-only`
/// are the staged bring-up rungs (see `linux/docs/first-run-observe.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnforceMode {
    /// Log what WOULD happen; apply nothing (no freeze, lock, VT, or web).
    Observe,
    /// Apply freeze/thaw only; no lock screen, no VT lock, no web filter.
    FreezeOnly,
    /// Full enforcement (freeze + lock + VT + web). Production default.
    Enforce,
}

impl EnforceMode {
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "observe" => EnforceMode::Observe,
            "freeze-only" | "freeze" => EnforceMode::FreezeOnly,
            _ => EnforceMode::Enforce,
        }
    }
    pub fn from_env() -> Self {
        Self::from_label(&std::env::var("CHARTER_ENFORCE").unwrap_or_default())
    }
    /// Freeze/thaw effects are applied to slices (FreezeOnly + Enforce).
    pub fn applies_effects(self) -> bool {
        !matches!(self, EnforceMode::Observe)
    }
    /// The lock screen + VT lock are driven (Enforce only).
    pub fn shows_lock(self) -> bool {
        matches!(self, EnforceMode::Enforce)
    }
}

fn source_label(s: &PolicySource) -> &'static str {
    match s {
        PolicySource::Guardian => "guardian",
        PolicySource::DeviceOnly => "device-only",
        PolicySource::Unconstrained => "unconstrained",
    }
}

/// Human-readable log lines for OBSERVE mode: one per child that WOULD be
/// enforced this tick (freeze and/or lock). Children with no effect are omitted
/// so the log stays quiet. Pure — the loop just prints the result.
pub fn observe_lines(decisions: &[ChildDecision], passwd: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for d in decisions {
        let would_freeze = d
            .effects
            .iter()
            .any(|e| matches!(e, EnforcerEffect::Freeze(_)));
        let lock_title = d.effects.iter().find_map(|e| match e {
            EnforcerEffect::ShowLock(r) if d.active && d.locked => Some(lock_message(*r).0),
            _ => None,
        });
        if !would_freeze && lock_title.is_none() {
            continue;
        }
        let name = user_for_uid(passwd, d.uid).unwrap_or_else(|| "?".into());
        let mut what: Vec<String> = Vec::new();
        if would_freeze {
            what.push("freeze".into());
        }
        if let Some(title) = lock_title {
            what.push(format!("lock: {title}"));
        }
        lines.push(format!(
            "observe: WOULD enforce uid {} ({}) — {} [source={}]",
            d.uid,
            name,
            what.join(" + "),
            source_label(&d.source),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_schedule::{FreezeTarget, LockReason};

    #[test]
    fn enforce_mode_from_label_maps_observe_freeze_else_enforce() {
        assert_eq!(EnforceMode::from_label("observe"), EnforceMode::Observe);
        assert_eq!(EnforceMode::from_label("OBSERVE"), EnforceMode::Observe);
        assert_eq!(
            EnforceMode::from_label(" freeze-only "),
            EnforceMode::FreezeOnly
        );
        assert_eq!(EnforceMode::from_label("freeze"), EnforceMode::FreezeOnly);
        assert_eq!(EnforceMode::from_label(""), EnforceMode::Enforce);
        assert_eq!(EnforceMode::from_label("enforce"), EnforceMode::Enforce);
        assert_eq!(EnforceMode::from_label("nonsense"), EnforceMode::Enforce);
    }

    #[test]
    fn enforce_mode_effect_and_lock_gating() {
        assert!(!EnforceMode::Observe.applies_effects());
        assert!(EnforceMode::FreezeOnly.applies_effects());
        assert!(EnforceMode::Enforce.applies_effects());
        assert!(!EnforceMode::Observe.shows_lock());
        assert!(!EnforceMode::FreezeOnly.shows_lock());
        assert!(EnforceMode::Enforce.shows_lock());
    }

    #[test]
    fn observe_lines_names_only_children_that_would_be_enforced() {
        let passwd = "bob:x:1001:1001:Bob:/home/bob:/bin/bash\n\
                      ada:x:1002:1002:Ada:/home/ada:/bin/bash\n";
        let decisions = vec![
            ChildDecision {
                uid: 1001,
                active: true,
                locked: true,
                reason: Some(LockReason::Budget),
                effects: vec![
                    EnforcerEffect::Freeze(FreezeTarget::ManagedAppSlice),
                    EnforcerEffect::ShowLock(LockReason::Budget),
                ],
                source: PolicySource::DeviceOnly,
            },
            ChildDecision {
                uid: 1002,
                active: false,
                locked: false,
                reason: None,
                effects: vec![],
                source: PolicySource::Guardian,
            },
        ];
        let lines = observe_lines(&decisions, passwd);
        assert_eq!(lines.len(), 1, "only the enforced child is logged");
        assert!(lines[0].contains("uid 1001"));
        assert!(lines[0].contains("bob"));
        assert!(lines[0].contains("freeze"));
        assert!(lines[0].contains("lock"));
        assert!(lines[0].contains("device-only"));
    }
}
