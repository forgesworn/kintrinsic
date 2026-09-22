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
    /// No `content` clause cached and nothing of ours is on disk — web content
    /// is unmanaged (no-op).
    Absent,
    /// The clause is gone and the restrictions we had written were RETRACTED:
    /// the unrestricted policy has just been materialised over them.
    Retracted,
    /// The clause store could not be read. NOT the same event as `Absent`:
    /// the last-known policy stays exactly where it is (see [`WebContentEnforcer::reconcile`]).
    Unavailable,
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
            // A retraction is a real change to what the ward's browser does,
            // so it is audited — unlike the two genuine no-ops.
            WebReconcile::Retracted => "retracted",
            WebReconcile::Absent | WebReconcile::Unavailable | WebReconcile::Unchanged => {
                return None
            }
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

/// Where the "we have written a web policy" marker lives. Its presence is the
/// only durable record that Kintrinsic, rather than the machine's owner, put
/// the file in `/etc/firefox/policies` — and a retraction that only remembers
/// in RAM is no retraction at all: the case B7 describes is a guardian who
/// deletes the clause, and the daemon that was carrying the memory gets
/// restarted (or the clause is deleted while it is down), after which the
/// restrictions sit on disk with nothing left to explain or remove them.
///
/// `/var/lib` (not `/run`): it must survive exactly the reboot that a purely
/// in-memory flag does not.
fn web_marker_path() -> String {
    std::env::var("CHARTER_WEB_MARKER")
        .unwrap_or_else(|_| "/var/lib/charter/web-policy.applied".into())
}

/// Stateful enforcer: remembers the last-applied policy for change-detection.
#[derive(Default)]
pub struct WebContentEnforcer {
    last_applied: Option<EffectiveWebPolicy>,
    /// Overridden only by tests, so the marker is per-fixture rather than a
    /// process-global env var several tests would race over.
    marker_path: Option<String>,
}

impl WebContentEnforcer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test seam: keep the durable marker inside a fixture's own temp dir.
    #[cfg(test)]
    fn with_marker_path(path: &str) -> Self {
        Self {
            last_applied: None,
            marker_path: Some(path.to_string()),
        }
    }

    fn marker(&self) -> String {
        self.marker_path.clone().unwrap_or_else(web_marker_path)
    }

    /// Have we ever materialised a policy that is still in force? In-memory
    /// first (this daemon's own run), the on-disk marker second (an earlier
    /// one's).
    fn has_live_policy(&self) -> bool {
        self.last_applied.is_some() || std::path::Path::new(&self.marker()).exists()
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
        // Durable "there is a Kintrinsic policy on this box" record, so a later
        // daemon run can still RETRACT what this one wrote.
        let _ = crate::atomic_file::atomic_write(&self.marker(), b"1\n", 0o600);
        WebReconcile::Enacted { locked }
    }

    /// B7: the clause is GONE. Anything we materialised for it must come back
    /// off, once — otherwise deleting the content clause leaves the ward's
    /// browser permanently restricted with no clause anywhere explaining why,
    /// and no surface saying so.
    ///
    /// "Once" matters both ways: on a host Kintrinsic has never configured
    /// there is nothing to retract and we must not touch the browser at all
    /// (that would stomp the machine owner's own `policies.json`), and once
    /// retracted we must not rewrite the same unrestricted document every
    /// reconcile.
    async fn retract<S: SystemLayer>(&mut self, sys: &S) -> WebReconcile {
        let unrestricted = EffectiveWebPolicy::unrestricted();
        if self.last_applied.as_ref() == Some(&unrestricted) {
            return WebReconcile::Absent; // already retracted this run
        }
        if !self.has_live_policy() {
            return WebReconcile::Absent; // never configured — hands off
        }
        match self.materialize(sys, unrestricted).await {
            WebReconcile::Enacted { .. } => {
                let _ = std::fs::remove_file(self.marker());
                eprintln!(
                    "charterd: the web-content clause is gone — the browser and DNS \
                     restrictions it had written have been retracted"
                );
                WebReconcile::Retracted
            }
            other => other,
        }
    }

    /// Load → evaluate → render → materialize, only when the effective policy
    /// changed. Fail-closed on an unparseable clause (locked policy); a removed
    /// clause RETRACTS; an unreadable store changes nothing.
    pub async fn reconcile<S: SystemLayer>(&mut self, sys: &S) -> WebReconcile {
        let clause_json = match sys.clauses().get_clause(ClauseKind::Content.store_key()) {
            Ok(Some(j)) => j,
            // The clause was REMOVED: retract what we wrote for it.
            Ok(None) => return self.retract(sys).await,
            // A store READ ERROR is a different event entirely, and conflating
            // the two is what made a transient EIO read as "the guardian set
            // nothing". Keep the last-known policy exactly where it is and say
            // nothing about it; the next reconcile will settle it.
            Err(_) => return WebReconcile::Unavailable,
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

    /// Curator ids are now required to be 64 lowercase hex (02-G3): anything
    /// shorter can never match a subscribed list, so a two-letter fixture id
    /// silently drops out of the quorum.
    const CURATOR_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1";
    const CURATOR_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2";

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
            &format!(
                r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{CURATOR_A}","{CURATOR_B}"],"quorumN":2,"issuedAt":1}}"#
            ),
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
            &format!(
                r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{CURATOR_A}","{CURATOR_B}"],"quorumN":2,"issuedAt":1}}"#
            ),
        );
        put_list(
            &sys,
            &format!("{CURATOR_A}:main"),
            5,
            &format!(
                r#"{{"curator":"{CURATOR_A}","entries":[{{"domain":"kids.example","rating":"KidSafe"}}]}}"#
            ),
        );
        put_list(
            &sys,
            &format!("{CURATOR_B}:main"),
            5,
            &format!(
                r#"{{"curator":"{CURATOR_B}","entries":[{{"domain":"kids.example","rating":"KidSafe"}}]}}"#
            ),
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
            &format!(
                r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{CURATOR_A}","{CURATOR_B}"],"quorumN":2,"issuedAt":1}}"#
            ),
        );
        // Only one curator rates solo.example — below quorum 2.
        put_list(
            &sys,
            &format!("{CURATOR_A}:main"),
            5,
            &format!(
                r#"{{"curator":"{CURATOR_A}","entries":[{{"domain":"solo.example","rating":"KidSafe"}}]}}"#
            ),
        );

        let mut e = WebContentEnforcer::new();
        e.reconcile(&sys).await;
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(
            !pol.contains("solo.example"),
            "a sub-quorum domain must not be admitted"
        );
    }

    /// A per-test marker path, so the durable half of the retraction memory is
    /// a fixture rather than the real `/var/lib/charter` file.
    fn marker(name: &str) -> String {
        let d =
            std::env::temp_dir().join(format!("charterd-web-marker-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("applied").to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn absent_clause_on_an_unconfigured_host_never_touches_the_browser() {
        let sys = MockSystem::new(1000);
        let m = marker("unconfigured");
        let mut e = WebContentEnforcer::with_marker_path(&m);
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Absent);
        assert!(
            sys.web_policy().last_policies().is_none(),
            "a host Kintrinsic never configured keeps its owner's own policies.json"
        );
        assert!(sys.dns_filter().last_plan().is_none());
    }

    /// B7: deleting the content clause must RETRACT the restrictions it wrote,
    /// and must do so exactly once.
    #[tokio::test]
    async fn removing_the_clause_retracts_the_policy_once() {
        let sys = MockSystem::new(1000);
        let m = marker("retract");
        let mut e = WebContentEnforcer::with_marker_path(&m);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","parentAllow":["kids.example"],"issuedAt":1}"#,
        );
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { locked: false }
        );
        assert!(sys
            .web_policy()
            .last_policies()
            .unwrap()
            .contains("kids.example"));

        // The guardian deletes the clause. (`ClauseStore` has no delete, so an
        // empty store over a fresh disk is how "the clause is gone" is staged —
        // what reaches `reconcile` is the `Ok(None)` either way.)
        let gone = MockSystem::new(1000);
        assert_eq!(e.reconcile(&gone).await, WebReconcile::Retracted);
        let pol = gone.web_policy().last_policies().unwrap();
        assert!(
            !pol.contains("kids.example"),
            "the restriction must be gone, not merely unexplained: {pol}"
        );
        assert!(!pol.contains("<all_urls>"), "retracted, not locked: {pol}");
        assert!(
            !std::path::Path::new(&m).exists(),
            "the durable marker goes with the policy it recorded"
        );
        // And only once — a steady absent clause is not a per-tick rewrite.
        assert_eq!(e.reconcile(&gone).await, WebReconcile::Absent);
    }

    /// The restart case, which is the one a RAM-only flag cannot cover: the
    /// clause is deleted while the daemon is down (or the daemon is restarted
    /// after), so the fresh enforcer has no memory at all — only the marker.
    #[tokio::test]
    async fn a_restarted_daemon_still_retracts_an_earlier_runs_policy() {
        let sys = MockSystem::new(1000);
        let m = marker("restart");
        {
            let mut first = WebContentEnforcer::with_marker_path(&m);
            put_content(
                &sys,
                r#"{"v":1,"posture":"blocklist","ageTier":"older","issuedAt":1}"#,
            );
            assert!(matches!(
                first.reconcile(&sys).await,
                WebReconcile::Enacted { .. }
            ));
        }
        assert!(std::path::Path::new(&m).exists());

        // New daemon, no in-memory memory at all, clause gone.
        let gone = MockSystem::new(1000);
        let mut restarted = WebContentEnforcer::with_marker_path(&m);
        assert_eq!(restarted.reconcile(&gone).await, WebReconcile::Retracted);
        assert!(!std::path::Path::new(&m).exists());
        assert!(gone.web_policy().last_policies().is_some());
    }

    /// A store read error is NOT "the guardian set nothing": the last-known
    /// policy stays, and nothing is rewritten.
    #[tokio::test]
    async fn an_unreadable_store_keeps_the_last_known_policy() {
        let sys = MockSystem::new(1000);
        let m = marker("unreadable");
        let mut e = WebContentEnforcer::with_marker_path(&m);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","parentAllow":["kids.example"],"issuedAt":1}"#,
        );
        assert!(matches!(
            e.reconcile(&sys).await,
            WebReconcile::Enacted { .. }
        ));
        sys.disk().break_clause_reads();
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Unavailable);
        assert!(
            sys.web_policy()
                .last_policies()
                .unwrap()
                .contains("kids.example"),
            "a transient read error must not read as an absent clause"
        );
        assert!(
            std::path::Path::new(&m).exists(),
            "nothing was retracted, so the marker stands"
        );
        let _ = std::fs::remove_dir_all(std::path::Path::new(&m).parent().unwrap());
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
