//! Device-side web content enforcement.
//!
//! Loads the cached `content` clause, evaluates it to an `EffectiveWebPolicy`,
//! renders the Firefox + DNS config artifacts, and materializes them through the
//! `WebPolicyOps` / `DnsFilterOps` ports. This is **idempotent state-sync** (like
//! schedule/budget enforcement), *not* a brokered request/grant op — so it never
//! touches `OpType`. Change-detected so a clause that didn't move isn't
//! re-written every reconcile.
//!
//! Fail-closed: an unparseable clause evaluates to a locked policy; a port
//! failure surfaces `Failed` so the daemon can force the lock-down policy.

use charter_content::{evaluate_content_json, CuratorList, EffectiveWebPolicy};
use charter_proto::clause::ClauseKind;
use charter_sys::effects::{DnsFilterOps, WebPolicyOps};
use charter_sys::persistence::{ClauseStore, CuratorListStore};
use charter_sys::SystemLayer;

/// The result of a reconcile pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebReconcile {
    /// No `content` clause cached — web content is unmanaged (no-op).
    Absent,
    /// Policy unchanged since the last apply — no-op.
    Unchanged,
    /// Policy materialized. `locked` = the browser is fully blocked.
    Enacted { locked: bool },
    /// A renderer or port failed — the caller should force the lock-down policy.
    Failed,
}

impl WebReconcile {
    /// Device-audit tags (kind 31000) for this outcome, or `None` for no-ops.
    /// Classification only — no URLs / content ever appear.
    pub fn audit_tags(&self) -> Option<Vec<Vec<String>>> {
        let outcome = match self {
            WebReconcile::Enacted { locked: true } => "locked",
            WebReconcile::Enacted { locked: false } => "enacted",
            WebReconcile::Failed => "failed",
            WebReconcile::Absent | WebReconcile::Unchanged => return None,
        };
        Some(vec![
            vec!["outcome".into(), outcome.into()],
            vec!["op".into(), "web.policy".into()],
        ])
    }
}

/// Load + deserialize the cached curator lists, skipping any that fail to parse.
fn cached_lists<S: SystemLayer>(sys: &S) -> Vec<CuratorList> {
    sys.curator_lists()
        .all_lists()
        .unwrap_or_default()
        .iter()
        .filter_map(|j| serde_json::from_str::<CuratorList>(j).ok())
        .collect()
}

/// Stateful enforcer: remembers the last-applied policy for change-detection.
#[derive(Default)]
pub struct WebContentEnforcer {
    last_applied: Option<EffectiveWebPolicy>,
}

impl WebContentEnforcer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Materialize `policy` through both ports. Returns the outcome and, on
    /// success, records it for change-detection.
    async fn materialize<S: SystemLayer>(
        &mut self,
        sys: &S,
        policy: EffectiveWebPolicy,
    ) -> WebReconcile {
        let policies_json = charter_webpolicy::render_firefox_policies(&policy).to_string();
        let plan = charter_webpolicy::render_dns_filter(&policy);
        let plan_json = match serde_json::to_string(&plan) {
            Ok(s) => s,
            Err(_) => return WebReconcile::Failed,
        };

        if sys
            .web_policy()
            .write_policies(&policies_json)
            .await
            .is_err()
        {
            return WebReconcile::Failed;
        }
        if sys.dns_filter().apply_plan(&plan_json).await.is_err() {
            return WebReconcile::Failed;
        }

        let locked = policy.locked;
        self.last_applied = Some(policy);
        WebReconcile::Enacted { locked }
    }

    /// Load → evaluate → render → materialize, only when the effective policy
    /// changed. Fail-closed on an unparseable clause (locked policy).
    pub async fn reconcile<S: SystemLayer>(&mut self, sys: &S) -> WebReconcile {
        let clause_json = match sys.clauses().get_clause(ClauseKind::Content.store_key()) {
            Ok(Some(j)) => j,
            // No clause, or a store read error: nothing to enforce here.
            Ok(None) | Err(_) => return WebReconcile::Absent,
        };
        let lists = cached_lists(sys);
        let policy = evaluate_content_json(&clause_json, &lists);

        if self.last_applied.as_ref() == Some(&policy) {
            return WebReconcile::Unchanged;
        }
        self.materialize(sys, policy).await
    }

    /// Force the fail-closed locked policy (browser fully blocked). Used by the
    /// daemon on startup before the first reconcile, or after a `Failed`.
    pub async fn force_lock<S: SystemLayer>(&mut self, sys: &S) -> WebReconcile {
        self.materialize(sys, EffectiveWebPolicy::locked()).await
    }

    /// True iff a web-content clause is present. The startup fail-closed lock is
    /// only meaningful when a policy actually exists — otherwise `force_lock`
    /// would block the browser with no reconcile to ever clear it (an `Absent`
    /// reconcile leaves the lock in place). On an unconfigured host the daemon
    /// must never touch the browser.
    pub fn is_configured<S: SystemLayer>(sys: &S) -> bool {
        matches!(
            sys.clauses().get_clause(ClauseKind::Content.store_key()),
            Ok(Some(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_sys::persistence::ClauseStore;
    use charter_sys::MockSystem;

    fn put_content(sys: &MockSystem, json: &str) {
        sys.clauses()
            .put_clause(ClauseKind::Content.store_key(), 1, json)
            .unwrap();
    }

    fn put_list(sys: &MockSystem, key: &str, created_at: u64, json: &str) {
        use charter_sys::persistence::CuratorListStore;
        sys.curator_lists().put_list(key, created_at, json).unwrap();
    }

    #[test]
    fn is_configured_reflects_content_clause_presence() {
        let sys = MockSystem::new(1000);
        assert!(
            !WebContentEnforcer::is_configured(&sys),
            "no content clause → unconfigured → never touch the browser"
        );
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"issuedAt":1}"#,
        );
        assert!(
            WebContentEnforcer::is_configured(&sys),
            "content clause present → fail-closed startup is meaningful"
        );
    }

    #[tokio::test]
    async fn allowlist_admits_quorum_domain_from_cached_lists() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"issuedAt":1}"#,
        );
        put_list(
            &sys,
            "aa:main",
            5,
            r#"{"curator":"aa","entries":[{"domain":"kids.example","rating":"KidSafe"}]}"#,
        );
        put_list(
            &sys,
            "bb:main",
            5,
            r#"{"curator":"bb","entries":[{"domain":"kids.example","rating":"KidSafe"}]}"#,
        );

        let mut e = WebContentEnforcer::new();
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { locked: false }
        );
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(
            pol.contains("kids.example"),
            "a quorum-admitted domain must reach the Firefox allowlist"
        );
    }

    #[tokio::test]
    async fn allowlist_excludes_sub_quorum_domain() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"issuedAt":1}"#,
        );
        // Only one curator rates solo.example — below quorum 2.
        put_list(
            &sys,
            "aa:main",
            5,
            r#"{"curator":"aa","entries":[{"domain":"solo.example","rating":"KidSafe"}]}"#,
        );

        let mut e = WebContentEnforcer::new();
        e.reconcile(&sys).await;
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(
            !pol.contains("solo.example"),
            "a sub-quorum domain must not be admitted"
        );
    }

    #[tokio::test]
    async fn absent_clause_is_noop() {
        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::new();
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Absent);
        assert!(sys.web_policy().last_policies().is_none());
        assert!(sys.dns_filter().last_plan().is_none());
    }

    #[tokio::test]
    async fn allowlist_clause_materializes_both_ports() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","parentAllow":["kids.example"],"issuedAt":1}"#,
        );
        let mut e = WebContentEnforcer::new();
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { locked: false }
        );
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(pol.contains("WebsiteFilter"));
        assert!(pol.contains("kids.example"));
        let plan = sys.dns_filter().last_plan().unwrap();
        assert!(plan.contains("allowlist"));
    }

    #[tokio::test]
    async fn unchanged_second_reconcile_is_noop() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"blocklist","ageTier":"older","issuedAt":1}"#,
        );
        let mut e = WebContentEnforcer::new();
        assert!(matches!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { .. }
        ));
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Unchanged);
    }

    #[tokio::test]
    async fn unparseable_clause_fails_closed_to_locked() {
        let sys = MockSystem::new(1000);
        put_content(&sys, "{ not valid json");
        let mut e = WebContentEnforcer::new();
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { locked: true }
        );
        // The Firefox doc blocks everything.
        assert!(sys
            .web_policy()
            .last_policies()
            .unwrap()
            .contains("<all_urls>"));
    }

    #[tokio::test]
    async fn force_lock_blocks_everything() {
        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::new();
        assert_eq!(
            e.force_lock(&sys).await,
            WebReconcile::Enacted { locked: true }
        );
        assert!(sys
            .web_policy()
            .last_policies()
            .unwrap()
            .contains("<all_urls>"));
    }

    #[test]
    fn audit_tags_are_classification_only() {
        let tags = WebReconcile::Enacted { locked: false }
            .audit_tags()
            .unwrap();
        assert!(tags.contains(&vec!["op".to_string(), "web.policy".to_string()]));
        assert!(tags.contains(&vec!["outcome".to_string(), "enacted".to_string()]));
        assert_eq!(
            WebReconcile::Enacted { locked: true }.audit_tags().unwrap()[0],
            vec!["outcome".to_string(), "locked".to_string()]
        );
        assert!(WebReconcile::Unchanged.audit_tags().is_none());
        assert!(WebReconcile::Absent.audit_tags().is_none());
    }
}
