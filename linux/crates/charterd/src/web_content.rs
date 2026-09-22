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

/// Where charterd writes the rendered Firefox machine-policy document — the
/// exact file [`WebPolicyOps::write_policies`] materialises, at production's
/// `RealWebPolicyOps` location. 03-B7 only ever *reads* this path back, to
/// recognise our own handwriting on a host upgraded from before the marker
/// existed; it is never written through here.
fn firefox_policies_default_path() -> String {
    std::env::var("CHARTER_FIREFOX_POLICIES")
        .unwrap_or_else(|_| "/etc/firefox/policies/policies.json".into())
}

/// Where charterd writes the rendered DNS-layer plan — see
/// [`firefox_policies_default_path`]; same read-back-only role, for the DNS
/// half of 03-B7's fingerprint.
fn dns_plan_default_path() -> String {
    std::env::var("CHARTER_DNS_PLAN").unwrap_or_else(|_| "/var/lib/charter/dns/plan.json".into())
}

/// 03-B7: the explicit marker key every policy document we write now carries,
/// so a later charterd run recognizes its own handwriting without depending on
/// the durable marker file existing (that file was only introduced once — an
/// install running the pre-marker daemon has real restrictions on disk and no
/// marker to say so).
const KINTRINSIC_MARKER_KEY: &str = "_kintrinsic";

/// The header comment one release stamped as the first line of every DNS plan.
///
/// It is READ, never written: a `//` line makes the document invalid JSON, and
/// "nothing in this repo parses it back" is a fact about this repo today, not
/// about the DNS responder on the other side of the file or about the next
/// person to write a tool against it. The fingerprint now travels as a
/// `_kintrinsic` key INSIDE the object — the same marker, the same
/// can't-collide-with-an-owner's-own-file property, and the file stays
/// parseable. This constant stays so a plan written by that one release is
/// still recognised as ours on upgrade.
const LEGACY_DNS_PLAN_COMMENT_HEADER: &str =
    "// kintrinsic-managed: charterd owns this file, do not edit it by hand";

/// 03-B7's fingerprint for `policies.json`: true iff this content is almost
/// certainly ours. Two ways to be ours: the explicit `_kintrinsic.managed`
/// marker key (every version from now on), or — the legacy case, for a file
/// written before that key existed — the exact static hardening block
/// `render_firefox_policies` has always emitted in every state (allowlist,
/// blocklist, locked, revoked). That block is a specific, unusual combination
/// (DoH force-disabled and locked, dev tools disabled, telemetry disabled,
/// ECH disabled, TRR mode pinned) that a machine owner's own hand-authored
/// `policies.json` is not going to reproduce by accident.
///
/// Pure — no filesystem access — so both branches are unit-testable without
/// touching `/etc`.
fn firefox_policies_is_ours(content: &str) -> bool {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(content) else {
        return false;
    };
    let Some(policies) = doc.get("policies") else {
        return false;
    };
    let managed = policies
        .get(KINTRINSIC_MARKER_KEY)
        .and_then(|k| k.get("managed"))
        .and_then(|m| m.as_bool());
    if managed == Some(true) {
        return true;
    }
    let b = |k: &str| policies.get(k) == Some(&serde_json::Value::Bool(true));
    let doh_off = policies.get("DNSOverHTTPS").and_then(|d| d.get("Enabled"))
        == Some(&serde_json::Value::Bool(false));
    let doh_locked = policies.get("DNSOverHTTPS").and_then(|d| d.get("Locked"))
        == Some(&serde_json::Value::Bool(true));
    b("BlockAboutConfig")
        && b("DisablePrivateBrowsing")
        && b("DisableDeveloperTools")
        && b("DisableSafeMode")
        && b("DisableTelemetry")
        && b("DisableFirefoxStudies")
        && b("DisableEncryptedClientHello")
        && doh_off
        && doh_locked
}

/// 03-B7's fingerprint for the DNS plan file. Three ways to be ours, newest
/// first: the explicit `_kintrinsic.managed` key inside the object (every
/// version from now on); the [`LEGACY_DNS_PLAN_COMMENT_HEADER`] line, from the
/// one release that stamped a comment; or — the oldest case — the whole file
/// parses as JSON carrying exactly the field names `render_dns_filter`'s
/// `DnsFilterPlan` has always serialized (a pre-marker install's file has no
/// marker at all, just that object). Pure for the same reason as
/// [`firefox_policies_is_ours`].
fn dns_plan_is_ours(content: &str) -> bool {
    if content.starts_with(LEGACY_DNS_PLAN_COMMENT_HEADER) {
        return true;
    }
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(content) else {
        return false;
    };
    let Some(obj) = doc.as_object() else {
        return false;
    };
    if obj
        .get(KINTRINSIC_MARKER_KEY)
        .and_then(|k| k.get("managed"))
        .and_then(|m| m.as_bool())
        == Some(true)
    {
        return true;
    }
    [
        "mode",
        "allowDomains",
        "blockDomains",
        "blockCategories",
        "allowExceptions",
        "safeSearch",
        "youtubeRestrict",
        "rewrites",
    ]
    .iter()
    .all(|k| obj.contains_key(*k))
}

/// Which of the two documents a materialize is allowed to write.
///
/// Enacting writes both. A RETRACTION on a box with no durable marker writes
/// only the ones whose own fingerprint says we wrote them — see
/// [`WebContentEnforcer::on_disk_fingerprints`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WriteTargets {
    policies: bool,
    dns: bool,
}

impl WriteTargets {
    fn both() -> Self {
        WriteTargets {
            policies: true,
            dns: true,
        }
    }
    fn any(self) -> bool {
        self.policies || self.dns
    }
}

/// Stateful enforcer: remembers the last-applied policy for change-detection.
#[derive(Default)]
pub struct WebContentEnforcer {
    last_applied: Option<EffectiveWebPolicy>,
    /// Overridden only by tests, so the marker is per-fixture rather than a
    /// process-global env var several tests would race over.
    marker_path: Option<String>,
    /// Test-only overrides for the two files 03-B7 reads back to fingerprint;
    /// same per-fixture reasoning as `marker_path`. Production always reads
    /// the real paths from [`firefox_policies_default_path`] /
    /// [`dns_plan_default_path`].
    #[cfg(test)]
    firefox_policies_path: Option<String>,
    #[cfg(test)]
    dns_plan_path: Option<String>,
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
            firefox_policies_path: None,
            dns_plan_path: None,
        }
    }

    /// Test seam for 03-B7: a fixture that also controls where the enforcer
    /// reads back the on-disk Firefox policy / DNS plan files it fingerprints,
    /// so the pre-marker-upgrade scenario can be staged without touching
    /// `/etc` or `/var/lib`.
    #[cfg(test)]
    fn with_paths(marker: &str, firefox_policies: &str, dns_plan: &str) -> Self {
        Self {
            last_applied: None,
            marker_path: Some(marker.to_string()),
            firefox_policies_path: Some(firefox_policies.to_string()),
            dns_plan_path: Some(dns_plan.to_string()),
        }
    }

    fn marker(&self) -> String {
        self.marker_path.clone().unwrap_or_else(web_marker_path)
    }

    fn firefox_policies_path(&self) -> String {
        #[cfg(test)]
        if let Some(p) = &self.firefox_policies_path {
            return p.clone();
        }
        firefox_policies_default_path()
    }

    fn dns_plan_path(&self) -> String {
        #[cfg(test)]
        if let Some(p) = &self.dns_plan_path {
            return p.clone();
        }
        dns_plan_default_path()
    }

    /// 03-B7: which of the two documents on disk look like our own
    /// handwriting, asked SEPARATELY of each. Reads are best-effort — a missing
    /// file reads as empty content, which neither fingerprint matches, so an
    /// unconfigured host (nothing there at all) correctly comes back
    /// `(false, false)`.
    ///
    /// Per file, not one verdict for both. Either file matching is enough to
    /// know that SOME earlier run of ours configured this box — the failure
    /// 03-B7 closes is a stranded restriction on a pre-marker upgrade, and
    /// requiring both to match would let one going missing strand whatever the
    /// other still enforces. But "we wrote one of these" is not "we wrote both
    /// of these", and the retraction that follows OVERWRITES what it touches.
    /// A box where a hardening baseline someone else owns sits at
    /// `policies.json` and only our DNS plan is ours would otherwise have that
    /// baseline silently replaced with an unrestricted document, by a daemon
    /// whose whole rule at this point is "hands off what isn't ours".
    fn on_disk_fingerprints(&self) -> WriteTargets {
        let policies = std::fs::read_to_string(self.firefox_policies_path()).unwrap_or_default();
        let plan = std::fs::read_to_string(self.dns_plan_path()).unwrap_or_default();
        WriteTargets {
            policies: firefox_policies_is_ours(&policies),
            dns: dns_plan_is_ours(&plan),
        }
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
        self.materialize_to(sys, policy, WriteTargets::both()).await
    }

    /// [`materialize`](Self::materialize), writing only the documents `targets`
    /// names. Everything but the retraction path passes `both`: a policy we are
    /// ENACTING is ours by definition, and both halves of it have to land or
    /// the browser and the resolver disagree about what is allowed.
    async fn materialize_to<S: SystemLayer>(
        &mut self,
        sys: &S,
        policy: EffectiveWebPolicy,
        targets: WriteTargets,
    ) -> WebReconcile {
        // Stamp our fingerprint into both documents as we write them — the
        // `_kintrinsic` key here and the header comment below are what
        // `firefox_policies_is_ours` / `dns_plan_is_ours` look for on a later
        // run that has no durable marker of its own yet (03-B7).
        let mut policies_doc = charter_webpolicy::render_firefox_policies(&policy);
        if let Some(policies) = policies_doc.get_mut("policies") {
            policies[KINTRINSIC_MARKER_KEY] = serde_json::json!({ "managed": true });
        }
        let policies_json = policies_doc.to_string();
        let plan = charter_webpolicy::render_dns_filter(&policy);
        // The marker rides INSIDE the object, so the plan stays valid JSON for
        // whatever reads it next. `_kintrinsic` is not a `DnsFilterPlan` field,
        // so it is added to the serialized value rather than to the struct.
        let plan_json = match serde_json::to_value(&plan) {
            Ok(mut v) => {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        KINTRINSIC_MARKER_KEY.to_string(),
                        serde_json::json!({ "managed": true }),
                    );
                }
                v.to_string()
            }
            Err(_) => return WebReconcile::Failed,
        };

        if targets.policies
            && sys
                .web_policy()
                .write_policies(&policies_json)
                .await
                .is_err()
        {
            return WebReconcile::Failed;
        }
        if targets.dns && sys.dns_filter().apply_plan(&plan_json).await.is_err() {
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
        // Which documents this retraction is allowed to overwrite. With a
        // marker on disk it is both: that marker is this daemon's own record
        // that it wrote both of them.
        let mut targets = WriteTargets::both();
        if !self.has_live_policy() {
            // 03-B7: an install upgraded from *before* the durable marker
            // existed has no marker file even though an earlier run really
            // did write restrictions — read as "never configured", that
            // strands them forever if the clause is deleted while this
            // daemon is down. Recognise our own handwriting still on disk
            // and backfill the marker once, so the retraction below runs
            // exactly as it would have if the marker had always been there.
            //
            // But only over the files whose OWN fingerprint says they are ours.
            // Without the marker there is nothing else vouching for them, and
            // an unrestricted document written over a file we did not write is
            // not a retraction, it is a deletion of somebody else's policy.
            let found = self.on_disk_fingerprints();
            if !found.any() {
                return WebReconcile::Absent; // genuinely never configured — hands off
            }
            let _ = crate::atomic_file::atomic_write(&self.marker(), b"1\n", 0o600);
            if !found.policies {
                eprintln!(
                    "charterd: retracting the web-content restrictions, but the browser policy \
                     file on this machine carries no Kintrinsic marker — leaving it untouched"
                );
            }
            if !found.dns {
                eprintln!(
                    "charterd: retracting the web-content restrictions, but the DNS plan file on \
                     this machine carries no Kintrinsic marker — leaving it untouched"
                );
            }
            targets = found;
        }
        match self.materialize_to(sys, unrestricted, targets).await {
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

    /// A fresh per-test fixture dir with the marker path plus two file paths
    /// standing in for `/etc/firefox/policies/policies.json` and the DNS plan
    /// file, for 03-B7's on-disk-fingerprint tests.
    fn fingerprint_fixture(name: &str) -> (String, String, String) {
        let d = std::env::temp_dir().join(format!(
            "charterd-web-fingerprint-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (
            d.join("applied").to_string_lossy().into_owned(),
            d.join("policies.json").to_string_lossy().into_owned(),
            d.join("plan.json").to_string_lossy().into_owned(),
        )
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

    // -- 03-B7: fingerprint pure functions ---------------------------------

    #[test]
    fn firefox_policies_is_ours_recognizes_the_explicit_marker() {
        let doc = r#"{"policies":{"BlockAboutConfig":true,"_kintrinsic":{"managed":true}}}"#;
        assert!(firefox_policies_is_ours(doc));
    }

    #[test]
    fn firefox_policies_is_ours_recognizes_the_legacy_hardening_shape() {
        // What every pre-marker install wrote: no `_kintrinsic` key, but the
        // full static hardening block is there.
        let doc = r#"{"policies":{
            "BlockAboutConfig":true,"DisablePrivateBrowsing":true,
            "DisableDeveloperTools":true,"DisableSafeMode":true,
            "DisableTelemetry":true,"DisableFirefoxStudies":true,
            "DisableEncryptedClientHello":true,
            "DNSOverHTTPS":{"Enabled":false,"Locked":true},
            "WebsiteFilter":{"Block":["<all_urls>"]}
        }}"#;
        assert!(firefox_policies_is_ours(doc));
    }

    #[test]
    fn firefox_policies_is_ours_rejects_a_foreign_or_absent_file() {
        assert!(!firefox_policies_is_ours(""), "absent file");
        assert!(
            !firefox_policies_is_ours(r#"{"policies":{"DisableAppUpdate":true}}"#),
            "a machine owner's own unrelated policies.json"
        );
        assert!(!firefox_policies_is_ours("{ not json"));
    }

    #[test]
    fn dns_plan_is_ours_recognizes_the_marker_key() {
        let doc = r#"{"mode":"locked","_kintrinsic":{"managed":true}}"#;
        assert!(dns_plan_is_ours(doc));
        // …and the plan carrying it is still valid JSON, which the `//` header
        // it replaced was not.
        assert!(serde_json::from_str::<serde_json::Value>(doc).is_ok());
    }

    #[test]
    fn dns_plan_is_ours_still_recognizes_the_one_release_that_wrote_a_comment() {
        let content = format!("{LEGACY_DNS_PLAN_COMMENT_HEADER}\n{{\"mode\":\"locked\"}}");
        assert!(dns_plan_is_ours(&content));
    }

    #[test]
    fn dns_plan_is_ours_recognizes_the_legacy_shape() {
        // What every pre-marker install wrote: the bare `DnsFilterPlan`
        // fields, no comment line at all.
        let doc = r#"{"mode":"blocklist","allowDomains":[],"blockDomains":["bad.example"],
            "blockCategories":[],"allowExceptions":[],"safeSearch":true,
            "youtubeRestrict":"off","rewrites":[]}"#;
        assert!(dns_plan_is_ours(doc));
    }

    #[test]
    fn dns_plan_is_ours_rejects_a_foreign_or_absent_file() {
        assert!(!dns_plan_is_ours(""), "absent file");
        assert!(
            !dns_plan_is_ours(r#"{"someOtherTool":true}"#),
            "an unrelated JSON file some other tool put there"
        );
        assert!(!dns_plan_is_ours("{ not json"));
    }

    // -- 03-B7: startup backfill on retract() -------------------------------

    /// The scenario the review describes: a host running charterd from before
    /// the durable marker existed already has real restrictions written to
    /// disk. The daemon is upgraded (or just restarted) with no marker file,
    /// the guardian's content clause is then found gone — the fingerprint on
    /// disk must be recognized as ours, the marker backfilled, and the
    /// restriction actually retracted rather than stranded forever.
    #[tokio::test]
    async fn pre_marker_upgrade_with_our_fingerprint_on_disk_retracts_instead_of_stranding() {
        let (m, policies_path, plan_path) = fingerprint_fixture("ours");
        // Legacy-shape files: our hardening block / our field names, but no
        // `_kintrinsic` key and no header comment — exactly what a pre-marker
        // daemon run left behind.
        std::fs::write(
            &policies_path,
            r#"{"policies":{
                "BlockAboutConfig":true,"DisablePrivateBrowsing":true,
                "DisableDeveloperTools":true,"DisableSafeMode":true,
                "DisableTelemetry":true,"DisableFirefoxStudies":true,
                "DisableEncryptedClientHello":true,
                "DNSOverHTTPS":{"Enabled":false,"Locked":true},
                "WebsiteFilter":{"Block":["<all_urls>"],"Exceptions":["*://kids.example/*"]}
            }}"#,
        )
        .unwrap();
        std::fs::write(
            &plan_path,
            r#"{"mode":"allowlist","allowDomains":["kids.example"],"blockDomains":[],
                "blockCategories":[],"allowExceptions":[],"safeSearch":true,
                "youtubeRestrict":"off","rewrites":[]}"#,
        )
        .unwrap();
        assert!(
            !std::path::Path::new(&m).exists(),
            "no marker yet — the upgrade case"
        );

        // No content clause: the guardian deleted it while the pre-marker
        // daemon was down (or between the upgrade and this run).
        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::with_paths(&m, &policies_path, &plan_path);
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Retracted,
            "the on-disk fingerprint must be recognized as ours and retracted, not stranded"
        );
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(
            !pol.contains("kids.example") && !pol.contains("<all_urls>"),
            "the unrestricted policy must actually have been materialized: {pol}"
        );
        assert!(
            !std::path::Path::new(&m).exists(),
            "the marker was backfilled and then removed again by the retraction it enabled"
        );
    }

    /// The other branch: on-disk content that is not ours (or nothing on disk
    /// at all) must never be treated as a stranded policy — that would stomp
    /// a machine owner's own `policies.json` on a host Kintrinsic never
    /// configured.
    #[tokio::test]
    async fn foreign_or_absent_disk_state_on_upgrade_stays_hands_off() {
        let (m, policies_path, plan_path) = fingerprint_fixture("foreign");
        std::fs::write(&policies_path, r#"{"policies":{"DisableAppUpdate":true}}"#).unwrap();
        // No DNS plan file at all.

        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::with_paths(&m, &policies_path, &plan_path);
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Absent,
            "a foreign file must not be read as ours"
        );
        assert!(
            sys.web_policy().last_policies().is_none(),
            "hands off: nothing must be written over the owner's own file"
        );
        assert!(sys.dns_filter().last_plan().is_none());
        assert!(
            !std::path::Path::new(&m).exists(),
            "no marker must be backfilled for a fingerprint that isn't ours"
        );
    }

    /// The mixed box, and the reason the backfill verdict is taken PER FILE.
    /// Kintrinsic's DNS plan is on disk (so this machine genuinely was
    /// configured by an earlier run, and the retraction must happen), but
    /// `policies.json` belongs to somebody else — an enterprise Firefox
    /// hardening baseline, a distro package, an admin's own file. A single
    /// "either matched" verdict would write our unrestricted document over
    /// that baseline and silently undo it.
    #[tokio::test]
    async fn a_retraction_leaves_a_foreign_file_alone_and_still_retracts_ours() {
        let (m, policies_path, plan_path) = fingerprint_fixture("mixed");
        // Not ours: a real hardening posture, none of our marker and none of
        // our exact block.
        std::fs::write(
            &policies_path,
            r#"{"policies":{"DisableAppUpdate":true,"DisableTelemetry":true,
                "Extensions":{"Locked":["ublock@example"]}}}"#,
        )
        .unwrap();
        // Ours: the legacy `DnsFilterPlan` shape a pre-marker run wrote.
        std::fs::write(
            &plan_path,
            r#"{"mode":"allowlist","allowDomains":["kids.example"],"blockDomains":[],
                "blockCategories":[],"allowExceptions":[],"safeSearch":true,
                "youtubeRestrict":"off","rewrites":[]}"#,
        )
        .unwrap();

        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::with_paths(&m, &policies_path, &plan_path);
        assert_eq!(
            e.reconcile(&sys).await,
            WebReconcile::Retracted,
            "our DNS plan is ours, so the retraction must still run"
        );
        assert!(
            sys.web_policy().last_policies().is_none(),
            "the foreign browser policy must not be overwritten by the retraction"
        );
        let plan = sys
            .dns_filter()
            .last_plan()
            .expect("our own DNS plan is retracted");
        assert!(
            !plan.contains("kids.example"),
            "the DNS plan must have been retracted to unrestricted: {plan}"
        );
    }

    /// And the mirror image, so neither file is privileged over the other.
    #[tokio::test]
    async fn a_retraction_leaves_a_foreign_dns_plan_alone_and_still_retracts_ours() {
        let (m, policies_path, plan_path) = fingerprint_fixture("mixed-dns");
        std::fs::write(
            &policies_path,
            r#"{"policies":{
                "BlockAboutConfig":true,"DisablePrivateBrowsing":true,
                "DisableDeveloperTools":true,"DisableSafeMode":true,
                "DisableTelemetry":true,"DisableFirefoxStudies":true,
                "DisableEncryptedClientHello":true,
                "DNSOverHTTPS":{"Enabled":false,"Locked":true},
                "WebsiteFilter":{"Block":["<all_urls>"]}
            }}"#,
        )
        .unwrap();
        std::fs::write(
            &plan_path,
            r#"{"someOtherResolver":{"upstream":"1.1.1.1"}}"#,
        )
        .unwrap();

        let sys = MockSystem::new(1000);
        let mut e = WebContentEnforcer::with_paths(&m, &policies_path, &plan_path);
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Retracted);
        assert!(
            sys.dns_filter().last_plan().is_none(),
            "the foreign DNS plan must not be overwritten by the retraction"
        );
        assert!(
            sys.web_policy().last_policies().is_some(),
            "our own browser policy is still retracted"
        );
    }
}
