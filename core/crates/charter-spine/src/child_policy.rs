//! Per-child policy resolution — **Signet-first** precedence.
//!
//! For each managed child the daemon must decide which schedule/budget governs
//! them. A paired guardian's per-child clauses (cached in the
//! [`ChildClauseStore`](charter_sys::persistence::ChildClauseStore)) are
//! **authoritative**; the local device-only [`DeviceLimits`] are the
//! **fallback** for a child the guardian has never set (unpaired, offline before
//! the first clause, or simply not yet adopted).
//!
//! **Whole-child precedence:** once the guardian has set *any* clause for a
//! child, the guardian owns that child entirely — an unset dimension means "no
//! constraint" (the guardian's intent), NOT a fall-through to device-only. This
//! matches the existing single-child semantics (absent budget = no cap) and
//! keeps a child from being governed by two parents' rules at once.
//!
//! **Malformed-clause asymmetry** (preserved from `enforcer_runtime`): a
//! present-but-unparseable **schedule** fail-SAFES to a paused schedule (that
//! child locks); a present-but-unparseable **budget** fail-OPENS to no cap (the
//! schedule remains the fail-safe gate). Both are scoped to the one child.
//!
//! Pure — no I/O, no clock — so every precedence branch is unit-tested.

use charter_proto::{ClauseKind, GrantLearning};
use charter_schedule::{GrantBudget, GrantSchedule, WeeklySchedule};
use charter_sys::persistence::ChildClauseStore;
use charter_sys::SystemLayer;

use crate::local_limits::{uid_for_user, ChildConfig, DeviceLimits};

/// Which source is governing a child this tick (surfaced to the status feed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    /// A paired guardian's per-child clauses.
    Guardian,
    /// The local device-only limits (no guardian clause for this child).
    DeviceOnly,
    /// Neither source set anything — unconstrained (existing absent semantics).
    Unconstrained,
}

/// The effective schedule + budget for one child, plus where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectivePolicy {
    pub schedule: Option<GrantSchedule>,
    pub budget: Option<GrantBudget>,
    /// The guardian's time-free learning apps (orthogonal to screen-time
    /// ownership: a learning clause alone does NOT flip the child to
    /// guardian-governed time). Malformed clause fails to `None` — Learning
    /// attribution then charges screen (fail-closed for the budget).
    pub learning: Option<GrantLearning>,
    pub source: PolicySource,
}

/// The fail-SAFE schedule used when a guardian schedule clause is present but
/// unparseable: `paused` blocks all time regardless of tz, so the child locks.
fn fail_safe_schedule() -> GrantSchedule {
    GrantSchedule {
        v: 1,
        tz: "UTC".into(),
        paused: Some(true),
        weekly: WeeklySchedule::default(),
        overrides: None,
        issued_at: 0,
    }
}

/// Resolve the effective policy for one child. `guardian_clauses` is the cached
/// `(kind, json)` set for the child's subject (empty if unbound / none received
/// yet); `device_only` is the local fallback (if any).
pub fn resolve_effective(
    guardian_clauses: &[(u16, String)],
    device_only: Option<&DeviceLimits>,
    device_learning: Option<&GrantLearning>,
) -> EffectivePolicy {
    // Learning never participates in time ownership, but its SOURCE follows
    // the doctrine: a guardian learning clause always wins; with none, a
    // guardian who time-governs the child suppresses device-only learning (a
    // local admin must not open time-free holes in the guardian's budget);
    // only an un-governed child falls back to the device-only learning list.
    let guardian_learning = guardian_clauses
        .iter()
        .find(|(k, _)| *k == ClauseKind::Learning.store_key())
        .and_then(|(_, json)| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|v| GrantLearning::from_value(&v).ok());
    let guardian_time_governs = guardian_clauses.iter().any(|(k, _)| {
        *k == ClauseKind::Schedule.store_key() || *k == ClauseKind::Budget.store_key()
    });
    let learning = match guardian_learning {
        Some(g) => Some(g),
        None if guardian_time_governs => None,
        None => device_learning.cloned(),
    };

    // 1) The guardian owns the child's SCREEN-TIME once they set a time clause
    //    (schedule or budget) for them. A non-time clause (e.g. per-child
    //    `content`) is orthogonal and must NOT trigger whole-child ownership —
    //    otherwise it would suppress the device-only curfew/cap and fail OPEN to
    //    unlimited time. (Body validity is irrelevant here: a present-but-
    //    malformed schedule still counts, so the fail-safe below still fires.)
    let has_time_clause = guardian_clauses.iter().any(|(k, _)| {
        *k == ClauseKind::Schedule.store_key() || *k == ClauseKind::Budget.store_key()
    });
    if has_time_clause {
        let mut schedule = None;
        let mut budget = None;
        for (kind, json) in guardian_clauses {
            if *kind == ClauseKind::Schedule.store_key() {
                // Malformed schedule fail-SAFES (paused → locks this child).
                schedule = Some(
                    serde_json::from_str::<GrantSchedule>(json)
                        .unwrap_or_else(|_| fail_safe_schedule()),
                );
            } else if *kind == ClauseKind::Budget.store_key() {
                // Malformed budget fail-OPENS (None → no cap; schedule still gates).
                budget = serde_json::from_str::<GrantBudget>(json).ok();
            }
        }
        return EffectivePolicy {
            schedule,
            budget,
            learning,
            source: PolicySource::Guardian,
        };
    }

    // 2) No guardian clause → device-only fallback, if configured.
    if let Some(limits) = device_only {
        return EffectivePolicy {
            schedule: Some(limits.to_schedule(1)),
            budget: Some(limits.to_budget(1)),
            learning,
            source: PolicySource::DeviceOnly,
        };
    }

    // 3) Neither → unconstrained (matches existing absent-clause behaviour).
    EffectivePolicy {
        schedule: None,
        budget: None,
        learning,
        source: PolicySource::Unconstrained,
    }
}

/// Resolve every managed child's effective policy for the enforcement tick. For
/// each loaded [`ChildConfig`], pull the guardian's per-child clauses (by
/// `subject`) from the [`ChildClauseStore`] and apply Signet-first precedence via
/// [`resolve_effective`]. Children whose username doesn't resolve to a uid in
/// `passwd` are skipped. Generic over the system layer, so the precedence
/// integration is mock-tested without a daemon/bus/relay.
pub fn resolve_child_policies<S: SystemLayer>(
    sys: &S,
    passwd: &str,
    configs: &[(String, ChildConfig)],
) -> Vec<(u32, EffectivePolicy)> {
    configs
        .iter()
        .filter_map(|(user, cfg)| {
            let uid = uid_for_user(passwd, user)?;
            // Guardian's per-child clauses (empty if unbound or none received yet).
            let guardian_clauses = match &cfg.subject {
                Some(subject) => sys.child_clauses().clauses_for(subject).unwrap_or_default(),
                None => Vec::new(),
            };
            Some((
                uid,
                resolve_effective(
                    &guardian_clauses,
                    cfg.limits.as_ref(),
                    cfg.learning.as_ref(),
                ),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceLimits {
        DeviceLimits {
            tz: "Europe/London".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 120,
            weekend: None,
        }
    }

    fn guardian_schedule_json() -> String {
        serde_json::to_string(&GrantSchedule {
            v: 1,
            tz: "America/New_York".into(),
            paused: None,
            weekly: WeeklySchedule::default(),
            overrides: None,
            issued_at: 500,
        })
        .unwrap()
    }

    fn guardian_budget_json(daily: u32) -> String {
        let b: GrantBudget = serde_json::from_str(&format!(
            r#"{{"v":1,"tz":"America/New_York","dailyMinutes":{daily},"issuedAt":500}}"#
        ))
        .unwrap();
        serde_json::to_string(&b).unwrap()
    }

    const SCHEDULE: u16 = 1;
    const BUDGET: u16 = 2;

    #[test]
    fn guardian_clauses_win_over_device_only() {
        let clauses = vec![
            (SCHEDULE, guardian_schedule_json()),
            (BUDGET, guardian_budget_json(60)),
        ];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        // Guardian's tz + cap, not the device-only one.
        assert_eq!(p.schedule.as_ref().unwrap().tz, "America/New_York");
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(60));
    }

    #[test]
    fn device_only_when_no_guardian_clause() {
        let p = resolve_effective(&[], Some(&device()), None);
        assert_eq!(p.source, PolicySource::DeviceOnly);
        assert_eq!(p.schedule.as_ref().unwrap().tz, "Europe/London");
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(120));
    }

    #[test]
    fn unconstrained_when_neither_source() {
        let p = resolve_effective(&[], None, None);
        assert_eq!(p.source, PolicySource::Unconstrained);
        assert!(p.schedule.is_none() && p.budget.is_none());
    }

    #[test]
    fn whole_child_partial_guardian_schedule_only_leaves_budget_unconstrained() {
        // Guardian set only a schedule — budget stays None (NOT device-only).
        let clauses = vec![(SCHEDULE, guardian_schedule_json())];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_some());
        assert!(
            p.budget.is_none(),
            "whole-child: an unset guardian dimension is unconstrained, not device-only"
        );
    }

    #[test]
    fn whole_child_partial_guardian_budget_only_leaves_schedule_unconstrained() {
        let clauses = vec![(BUDGET, guardian_budget_json(45))];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_none());
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(45));
    }

    #[test]
    fn malformed_guardian_schedule_fail_safes_to_paused() {
        let clauses = vec![(SCHEDULE, "not json at all".to_string())];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert_eq!(
            p.schedule.as_ref().unwrap().paused,
            Some(true),
            "a broken guardian schedule must lock the child (fail-safe)"
        );
    }

    #[test]
    fn content_only_guardian_clause_does_not_suppress_device_only() {
        // A per-child CONTENT clause (kind 3) governs web filtering, NOT
        // screen-time. It must not trip whole-child ownership and strip the
        // device-only curfew/cap — that would fail OPEN to unlimited time.
        const CONTENT: u16 = 3;
        let clauses = vec![(CONTENT, r#"{"any":"content"}"#.to_string())];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(
            p.source,
            PolicySource::DeviceOnly,
            "a content clause must not suppress device-only screen-time limits"
        );
        assert!(p.schedule.is_some() && p.budget.is_some());
    }

    #[test]
    fn content_clause_alongside_guardian_schedule_still_guardian() {
        // But a real time clause (+ an incidental content clause) is still
        // guardian-owned.
        const CONTENT: u16 = 3;
        let clauses = vec![
            (SCHEDULE, guardian_schedule_json()),
            (CONTENT, r#"{"any":"content"}"#.to_string()),
        ];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_some());
    }

    #[test]
    fn malformed_guardian_budget_fail_opens_to_no_cap() {
        let clauses = vec![
            (SCHEDULE, guardian_schedule_json()),
            (BUDGET, "garbage".to_string()),
        ];
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(
            p.budget.is_none(),
            "a broken guardian budget fails open (schedule still gates)"
        );
        // The schedule is intact and NOT the fail-safe paused one.
        assert_eq!(p.schedule.as_ref().unwrap().paused, None);
    }
    fn khan_learning() -> charter_proto::GrantLearning {
        charter_proto::GrantLearning::from_value(&serde_json::json!({
            "v": 1, "issuedAt": 5,
            "apps": [{"id": "khan-academy", "label": "Khan Academy", "kind": "site",
                       "url": "https://www.khanacademy.org/", "domains": ["khanacademy.org"]}]
        }))
        .unwrap()
    }

    #[test]
    fn device_learning_applies_to_a_device_only_child() {
        let p = resolve_effective(&[], Some(&device()), Some(&khan_learning()));
        assert_eq!(p.source, PolicySource::DeviceOnly);
        assert!(
            p.learning.is_some(),
            "device learning rides device-only time"
        );
        // And to an unconstrained child (no time rules at all).
        let p2 = resolve_effective(&[], None, Some(&khan_learning()));
        assert!(p2.learning.is_some());
    }

    #[test]
    fn guardian_time_governance_suppresses_device_learning() {
        // Whole-child doctrine: once the guardian time-governs, a local
        // admin's device learning must not create time-free holes in the
        // guardian's budget. Guardian schedule clause present, no guardian
        // learning clause, device learning set → NO learning in force.
        let sched = guardian_schedule_json();
        let clauses = vec![(ClauseKind::Schedule.store_key(), sched)];
        let p = resolve_effective(&clauses, Some(&device()), Some(&khan_learning()));
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.learning.is_none());
    }

    #[test]
    fn guardian_learning_clause_beats_device_learning() {
        let mut g = khan_learning();
        g.cap_minutes = Some(45);
        let ljson = serde_json::to_string(&g).unwrap();
        let clauses = vec![(ClauseKind::Learning.store_key(), ljson)];
        let mut d = khan_learning();
        d.cap_minutes = Some(999);
        let p = resolve_effective(&clauses, Some(&device()), Some(&d));
        assert_eq!(p.learning.as_ref().unwrap().cap_minutes, Some(45));
    }
}

#[cfg(all(test, feature = "mock"))]
mod sys_tests {
    use super::*;
    use crate::local_limits::ChildConfig;
    use charter_sys::persistence::ChildClauseStore;
    use charter_sys::{MockSystem, SystemLayer};

    const PASSWD: &str =
        "root:x:0:0::/root:/bin/bash\nalice:x:1001:1001::/home/alice:/bin/bash\nbob:x:1002:1002::/home/bob:/bin/bash\n";
    const SUBJ_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn dl() -> DeviceLimits {
        DeviceLimits {
            tz: "UTC".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 60,
            weekend: None,
        }
    }

    fn store_guardian_schedule(sys: &MockSystem, subject: &str) {
        let sched = serde_json::to_string(&GrantSchedule {
            v: 1,
            tz: "America/New_York".into(),
            paused: None,
            weekly: WeeklySchedule::default(),
            overrides: None,
            issued_at: 100,
        })
        .unwrap();
        sys.child_clauses()
            .put_child_clause(subject, ClauseKind::Schedule.store_key(), 100, &sched)
            .unwrap();
    }

    fn bound(subject: &str, limits: Option<DeviceLimits>) -> ChildConfig {
        ChildConfig {
            subject: Some(subject.into()),
            limits,
            learning: None,
        }
    }

    #[test]
    fn bound_child_with_guardian_clause_uses_guardian() {
        let sys = MockSystem::new(1000);
        store_guardian_schedule(&sys, SUBJ_A);
        let configs = vec![("alice".to_string(), bound(SUBJ_A, Some(dl())))];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 1001, "username resolved to uid");
        assert_eq!(
            out[0].1.source,
            PolicySource::Guardian,
            "guardian clause supersedes the device-only fallback"
        );
        assert_eq!(out[0].1.schedule.as_ref().unwrap().tz, "America/New_York");
    }

    #[test]
    fn bound_child_without_clause_falls_back_to_device_only() {
        let sys = MockSystem::new(1000);
        // Subject bound, but no clause stored yet (offline / not-yet-adopted).
        let configs = vec![("alice".to_string(), bound(SUBJ_A, Some(dl())))];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        assert_eq!(out[0].1.source, PolicySource::DeviceOnly);
    }

    #[test]
    fn unbound_child_uses_device_only() {
        let sys = MockSystem::new(1000);
        let configs = vec![(
            "bob".to_string(),
            ChildConfig {
                subject: None,
                limits: Some(dl()),
                learning: None,
            },
        )];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        assert_eq!(out[0].0, 1002);
        assert_eq!(out[0].1.source, PolicySource::DeviceOnly);
    }

    #[test]
    fn bound_child_no_clause_no_limits_is_unconstrained() {
        let sys = MockSystem::new(1000);
        let configs = vec![("alice".to_string(), bound(SUBJ_A, None))];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        assert_eq!(out[0].1.source, PolicySource::Unconstrained);
    }

    #[test]
    fn unknown_username_is_skipped() {
        let sys = MockSystem::new(1000);
        let configs = vec![(
            "ghost".to_string(),
            ChildConfig {
                subject: None,
                limits: Some(dl()),
                learning: None,
            },
        )];
        assert!(resolve_child_policies(&sys, PASSWD, &configs).is_empty());
    }

    #[test]
    fn mixed_children_resolve_independently() {
        let sys = MockSystem::new(1000);
        store_guardian_schedule(&sys, SUBJ_A); // alice adopted by guardian
        let configs = vec![
            ("alice".to_string(), bound(SUBJ_A, Some(dl()))),
            (
                "bob".to_string(),
                ChildConfig {
                    subject: None,
                    limits: Some(dl()),
                    learning: None,
                },
            ),
        ];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        let alice = out.iter().find(|(u, _)| *u == 1001).unwrap();
        let bob = out.iter().find(|(u, _)| *u == 1002).unwrap();
        assert_eq!(alice.1.source, PolicySource::Guardian);
        assert_eq!(bob.1.source, PolicySource::DeviceOnly);
    }
}
