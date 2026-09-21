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
//! **A clause that is there and cannot be used fails SAFE, on every
//! dimension.** A present-but-unparseable **schedule** becomes a paused
//! schedule and a present-but-unparseable **budget** becomes a zero-minute
//! one, so either way that child locks and the guardian can see that something
//! needs fixing. Both are scoped to the one child.
//!
//! The budget used to fail OPEN here, justified by "the schedule remains the
//! fail-safe gate". That argument has a hole: it only holds if a schedule
//! clause exists. A guardian who sets "2 hours a day, any time" and no
//! schedule — an entirely ordinary setup — got ZERO enforcement the moment the
//! budget blob became unreadable, with no lock, no audit and no reason
//! reported, indefinitely. An unreadable clause must never be quieter than a
//! readable one.
//!
//! Learning is the exception that proves the rule and is unchanged: it is the
//! one dimension that CREATES free time, so its fail-safe is `None` — nothing
//! is time-free, and the ward is charged for everything.
//!
//! Pure — no I/O, no clock — so every precedence branch is unit-tested.

use charter_proto::{ClauseKind, GrantLearning};
use charter_schedule::{GrantBudget, GrantSchedule, WeeklySchedule};
use charter_sys::persistence::{ChildClauseStore, ChildClauses};
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

/// The tz a synthesised fail-safe carries. Whatever the guardian meant is
/// exactly what we could not read, so there is no better answer — and none is
/// needed: both syntheses block all time in every tz.
pub(crate) const FAIL_SAFE_TZ: &str = "UTC";

/// The fail-SAFE schedule used when a guardian schedule clause is present but
/// unparseable: `paused` blocks all time regardless of tz, so the child locks.
pub(crate) fn fail_safe_schedule(tz: &str) -> GrantSchedule {
    GrantSchedule {
        v: 1,
        tz: tz.to_string(),
        paused: Some(true),
        weekly: WeeklySchedule::default(),
        overrides: None,
        issued_at: 0,
    }
}

/// The fail-SAFE budget used when a guardian budget clause is present but
/// cannot be read or parsed: nothing left on either cap today. The mirror of
/// [`fail_safe_schedule`], and for the same reason — a clause we cannot use
/// must never be quieter than one we can.
///
/// `paused` alone is not enough. `quota_parts_signed_pooled` answers a paused
/// budget with "nothing left" only on the caps that are actually SET, so a
/// paused budget carrying no `dailyMinutes`/`weeklyMinutes` reads as
/// unbounded — the exact fail-open this is here to prevent. Both caps are
/// therefore named at zero as well.
///
/// A paused budget also ignores every extension pool, so this cannot be
/// undone by a grant that happens to be in flight: the way out is a readable
/// budget clause, which is the thing that actually needs fixing.
pub(crate) fn fail_safe_budget(tz: &str) -> GrantBudget {
    GrantBudget {
        v: 1,
        tz: tz.to_string(),
        daily_minutes: Some(0),
        weekly_minutes: Some(0),
        week_start: None,
        paused: Some(true),
        revoked: None,
        model: None,
        issued_at: 0,
    }
}

/// Resolve the effective policy for one child. `guardian_clauses` is the cached
/// clause set for the child's subject (empty if unbound / none received yet),
/// together with the kinds whose slot is present-but-unreadable;
/// `device_only` is the local fallback (if any).
pub fn resolve_effective(
    guardian_clauses: &ChildClauses,
    device_only: Option<&DeviceLimits>,
    device_learning: Option<&GrantLearning>,
) -> EffectivePolicy {
    let clauses = &guardian_clauses.clauses;
    let unreadable = |kind: ClauseKind| guardian_clauses.unreadable.contains(&kind.store_key());

    // Learning never participates in time ownership, but its SOURCE follows
    // the doctrine: a guardian learning clause always wins; with none, a
    // guardian who time-governs the child suppresses device-only learning (a
    // local admin must not open time-free holes in the guardian's budget);
    // only an un-governed child falls back to the device-only learning list.
    let guardian_learning = clauses
        .iter()
        .find(|(k, _)| *k == ClauseKind::Learning.store_key())
        .and_then(|(_, json)| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|v| GrantLearning::from_value(&v).ok());
    // An unreadable time clause governs exactly as a readable one does: the
    // guardian HAS set something for this child, we simply cannot see what.
    let guardian_time_governs = clauses.iter().any(|(k, _)| {
        *k == ClauseKind::Schedule.store_key() || *k == ClauseKind::Budget.store_key()
    }) || unreadable(ClauseKind::Schedule)
        || unreadable(ClauseKind::Budget);
    let learning = if unreadable(ClauseKind::Learning) {
        // Fail-CLOSED, the same direction a learning clause that will not
        // parse already takes: no list means nothing is time-free, so the ward
        // is charged for everything. Falling through to the device-only list
        // would open time-free holes on the strength of a file we could not
        // read.
        None
    } else {
        match guardian_learning {
            Some(g) => Some(g),
            None if guardian_time_governs => None,
            None => device_learning.cloned(),
        }
    };

    // 1) The guardian owns the child's SCREEN-TIME once they set a time clause
    //    (schedule or budget) for them. A non-time clause (e.g. per-child
    //    `content`) is orthogonal and must NOT trigger whole-child ownership —
    //    otherwise it would suppress the device-only curfew/cap and fail OPEN to
    //    unlimited time. (Body validity is irrelevant here: a present-but-
    //    malformed schedule still counts, so the fail-safe below still fires.)
    if guardian_time_governs {
        let mut schedule = None;
        let mut budget = None;
        for (kind, json) in clauses {
            if *kind == ClauseKind::Schedule.store_key() {
                // Malformed schedule fail-SAFES (paused → locks this child).
                schedule = Some(
                    serde_json::from_str::<GrantSchedule>(json)
                        .unwrap_or_else(|_| fail_safe_schedule(FAIL_SAFE_TZ)),
                );
            } else if *kind == ClauseKind::Budget.store_key() {
                // Malformed budget fail-SAFES too (zero cap → locks this
                // child). It used to fail OPEN on the argument that the
                // schedule still gates — which is only true for a child who
                // HAS a schedule. See the module doc.
                budget = Some(
                    serde_json::from_str::<GrantBudget>(json)
                        .unwrap_or_else(|_| fail_safe_budget(FAIL_SAFE_TZ)),
                );
            }
        }
        // A slot we could not read takes the SAME branch a body we could not
        // parse takes — the two are the same event as far as this child is
        // concerned, and the one that reads as "the guardian set nothing" is
        // the dangerous one.
        if unreadable(ClauseKind::Schedule) {
            schedule = Some(fail_safe_schedule(FAIL_SAFE_TZ));
        }
        if unreadable(ClauseKind::Budget) {
            budget = Some(fail_safe_budget(FAIL_SAFE_TZ));
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
                Some(subject) => match sys.child_clauses().clauses_for(subject) {
                    Ok(c) => c,
                    // The walk itself failed: the directory is there and will
                    // not be listed (an EIO, a permissions change). That says
                    // nothing about any one kind, so the two dimensions that
                    // actually gate time are both treated as unreadable.
                    // `unwrap_or_default()` here used to mean "the guardian
                    // set nothing", which is unlimited time on a disk fault.
                    Err(e) => {
                        eprintln!(
                            "charter: cannot list the clause directory for uid {uid} ({e}) — \
                             locking on schedule and budget until it reads again"
                        );
                        ChildClauses {
                            clauses: Vec::new(),
                            unreadable: vec![
                                ClauseKind::Schedule.store_key(),
                                ClauseKind::Budget.store_key(),
                            ],
                        }
                    }
                },
                None => ChildClauses::default(),
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

    /// The ordinary case: everything the guardian set for this child read back
    /// cleanly. Tests that need the other case name the unreadable kinds.
    fn readable(clauses: Vec<(u16, String)>) -> ChildClauses {
        ChildClauses::from(clauses)
    }

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
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        // Guardian's tz + cap, not the device-only one.
        assert_eq!(p.schedule.as_ref().unwrap().tz, "America/New_York");
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(60));
    }

    #[test]
    fn device_only_when_no_guardian_clause() {
        let p = resolve_effective(&readable(vec![]), Some(&device()), None);
        assert_eq!(p.source, PolicySource::DeviceOnly);
        assert_eq!(p.schedule.as_ref().unwrap().tz, "Europe/London");
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(120));
    }

    #[test]
    fn unconstrained_when_neither_source() {
        let p = resolve_effective(&readable(vec![]), None, None);
        assert_eq!(p.source, PolicySource::Unconstrained);
        assert!(p.schedule.is_none() && p.budget.is_none());
    }

    #[test]
    fn whole_child_partial_guardian_schedule_only_leaves_budget_unconstrained() {
        // Guardian set only a schedule — budget stays None (NOT device-only).
        let clauses = vec![(SCHEDULE, guardian_schedule_json())];
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
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
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_none());
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(45));
    }

    #[test]
    fn malformed_guardian_schedule_fail_safes_to_paused() {
        let clauses = vec![(SCHEDULE, "not json at all".to_string())];
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
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
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
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
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_some());
    }

    #[test]
    fn malformed_guardian_budget_fail_safes_to_a_zero_cap() {
        let clauses = vec![
            (SCHEDULE, guardian_schedule_json()),
            (BUDGET, "garbage".to_string()),
        ];
        let p = resolve_effective(&readable(clauses), Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        let b = p
            .budget
            .as_ref()
            .expect("a broken guardian budget must not read as no cap at all");
        assert_eq!((b.paused, b.daily_minutes), (Some(true), Some(0)));
        // The schedule is intact and NOT the fail-safe paused one: one bad
        // dimension never drags the other down with it.
        assert_eq!(p.schedule.as_ref().unwrap().paused, None);
    }

    #[test]
    fn a_budget_only_child_with_a_malformed_budget_is_not_left_unenforced() {
        // The hole in the old "the schedule remains the fail-safe gate"
        // argument: there is no schedule here. `dailyMinutes: -5` against
        // `Option<u32>` is enough to get to this state, and it used to mean
        // schedule None, budget None, effective -1, locked false, forever.
        let clauses = vec![(
            BUDGET,
            r#"{"v":1,"tz":"Europe/London","dailyMinutes":-5,"issuedAt":500}"#.to_string(),
        )];
        let p = resolve_effective(&readable(clauses), None, None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert!(p.schedule.is_none(), "this child genuinely has no schedule");
        let b = p.budget.as_ref().expect("and their cap must still bite");
        assert_eq!(
            (b.paused, b.daily_minutes, b.weekly_minutes),
            (Some(true), Some(0), Some(0))
        );

        // And the enforcer agrees: nothing left, so the child is locked.
        let rem = charter_schedule::compute_remaining(&charter_schedule::EnforcerInputs {
            now_unix: 1_700_000_000,
            schedule: None,
            budget: p.budget.as_ref(),
            usage: &charter_schedule::UsageLedger::new(
                "UTC",
                charter_schedule::WeekStart::Mon,
                1_700_000_000,
            ),
            extension: &charter_schedule::ExtensionLedger::new("UTC", 1_700_000_000),
            consolidated: None,
            stand_down: None,
        });
        assert_eq!(rem.budget_secs, 0);
        assert!(rem.locked, "a budget-only child with an unusable cap locks");
    }

    #[test]
    fn a_budget_only_child_with_no_budget_at_all_is_still_unconstrained() {
        // The control, and the line the fail-safe must not cross: absent is
        // not malformed. A guardian who set nothing has lost nothing.
        let p = resolve_effective(&readable(vec![]), None, None);
        assert_eq!(p.source, PolicySource::Unconstrained);
        assert!(p.schedule.is_none() && p.budget.is_none());
    }

    /// A clause set where `unreadable` names kinds whose file is on disk and
    /// illegible — the state a truncated `<kind>.json` leaves behind.
    fn with_unreadable(clauses: Vec<(u16, String)>, unreadable: Vec<u16>) -> ChildClauses {
        ChildClauses {
            clauses,
            unreadable,
        }
    }

    #[test]
    fn an_unreadable_budget_leaves_the_schedule_enforced_and_locks_the_budget() {
        // The failure: one torn `2.json` used to take the child's schedule
        // with it, because the whole walk aborted and the caller read the
        // error as "no clauses" — no curfew, no cap, no lock, indefinitely.
        let clauses = with_unreadable(vec![(SCHEDULE, guardian_schedule_json())], vec![BUDGET]);
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert_eq!(
            p.schedule.as_ref().unwrap().tz,
            "America/New_York",
            "the readable schedule is still the guardian's own, unaltered"
        );
        assert_eq!(
            p.schedule.as_ref().unwrap().paused,
            None,
            "and it is NOT the fail-safe paused one — only the budget was unreadable"
        );
        let b = p
            .budget
            .as_ref()
            .expect("an unreadable budget is not an absent one");
        assert_eq!(b.paused, Some(true));
        assert_eq!(
            (b.daily_minutes, b.weekly_minutes),
            (Some(0), Some(0)),
            "paused with no caps set reads as unbounded, so both caps are named at zero"
        );
    }

    #[test]
    fn an_unreadable_schedule_locks_the_child_and_keeps_a_readable_budget() {
        let clauses = with_unreadable(vec![(BUDGET, guardian_budget_json(60))], vec![SCHEDULE]);
        let p = resolve_effective(&clauses, Some(&device()), None);
        assert_eq!(p.source, PolicySource::Guardian);
        assert_eq!(p.schedule.as_ref().unwrap().paused, Some(true));
        assert_eq!(p.budget.as_ref().unwrap().daily_minutes, Some(60));
    }

    #[test]
    fn an_unreadable_time_clause_still_makes_the_guardian_the_owner() {
        // Nothing readable at all, and a device-only fallback sitting there.
        // Falling back to device-only would be defensible; falling through to
        // Unconstrained (what an `unwrap_or_default()`ed walk produced) is
        // unlimited time, and that is the branch this pins shut.
        let p = resolve_effective(
            &with_unreadable(vec![], vec![SCHEDULE]),
            Some(&device()),
            None,
        );
        assert_eq!(p.source, PolicySource::Guardian);
        assert_eq!(p.schedule.as_ref().unwrap().paused, Some(true));
    }

    #[test]
    fn an_unreadable_learning_clause_does_not_fall_back_to_device_learning() {
        // Learning is the one dimension that CREATES free time, so a list we
        // cannot read must mean no free time — not the local admin's list.
        const LEARNING: u16 = 5;
        let p = resolve_effective(
            &with_unreadable(vec![], vec![LEARNING]),
            Some(&device()),
            Some(&khan_learning()),
        );
        assert!(p.learning.is_none());
        // …and an unreadable learning clause alone does not seize the child's
        // screen-time: it is orthogonal, exactly as a readable one is.
        assert_eq!(p.source, PolicySource::DeviceOnly);
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
        let p = resolve_effective(&readable(vec![]), Some(&device()), Some(&khan_learning()));
        assert_eq!(p.source, PolicySource::DeviceOnly);
        assert!(
            p.learning.is_some(),
            "device learning rides device-only time"
        );
        // And to an unconstrained child (no time rules at all).
        let p2 = resolve_effective(&readable(vec![]), None, Some(&khan_learning()));
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
        let p = resolve_effective(&readable(clauses), Some(&device()), Some(&khan_learning()));
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
        let p = resolve_effective(&readable(clauses), Some(&device()), Some(&d));
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
    fn a_clause_directory_that_will_not_list_locks_rather_than_unconstrains() {
        // The walk itself failing (EIO, a permissions change) says nothing
        // about any one kind — but it used to be `unwrap_or_default()`ed into
        // "the guardian set nothing", which on a child with a curfew and a cap
        // is unlimited time for as long as the fault lasts.
        let sys = MockSystem::new(1000);
        store_guardian_schedule(&sys, SUBJ_A);
        sys.disk().break_clause_reads();
        let configs = vec![("alice".to_string(), bound(SUBJ_A, Some(dl())))];
        let out = resolve_child_policies(&sys, PASSWD, &configs);
        assert_eq!(out[0].1.source, PolicySource::Guardian);
        assert_eq!(
            out[0].1.schedule.as_ref().unwrap().paused,
            Some(true),
            "both enforcing dimensions take their fail-safe while the store is unreadable"
        );
        assert_eq!(
            out[0].1.budget.as_ref().unwrap().daily_minutes,
            Some(0),
            "and the budget too — a schedule-less child would otherwise be uncapped"
        );
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
