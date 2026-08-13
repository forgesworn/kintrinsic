//! The pure content evaluator: `GrantContent` + verified curator lists →
//! `EffectiveWebPolicy`. No I/O, no clock. Deterministic (`BTreeSet`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::clause::{GrantContent, Posture, YoutubeRestrict};
use crate::curator::{CuratorList, Rating};
use crate::domain::parse_domain;

/// The materializable effective decision for a child's web access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveWebPolicy {
    /// `None` when revoked / no constraint.
    pub posture: Option<Posture>,
    /// Paused or fail-closed → block all web.
    pub locked: bool,
    pub allow_domains: BTreeSet<String>,
    pub block_domains: BTreeSet<String>,
    pub block_categories: BTreeSet<String>,
    /// Parent-allowed domains, surfaced so the DNS layer can apply them as
    /// exceptions ON TOP OF category blocks (categories expand to domains only
    /// at that layer). In allowlist posture these are also folded into
    /// `allow_domains`.
    pub allow_exceptions: BTreeSet<String>,
    pub safe_search: bool,
    pub youtube_restrict: YoutubeRestrict,
}

impl EffectiveWebPolicy {
    /// Fail-closed: web fully blocked (unparseable clause or `paused`).
    pub fn locked() -> Self {
        EffectiveWebPolicy {
            posture: None,
            locked: true,
            allow_domains: BTreeSet::new(),
            block_domains: BTreeSet::new(),
            block_categories: BTreeSet::new(),
            allow_exceptions: BTreeSet::new(),
            safe_search: true,
            youtube_restrict: YoutubeRestrict::Off,
        }
    }

    /// No constraint (revoked): web unrestricted.
    pub fn unrestricted() -> Self {
        EffectiveWebPolicy {
            posture: None,
            locked: false,
            allow_domains: BTreeSet::new(),
            block_domains: BTreeSet::new(),
            block_categories: BTreeSet::new(),
            allow_exceptions: BTreeSet::new(),
            safe_search: false,
            youtube_restrict: YoutubeRestrict::Off,
        }
    }
}

/// Parse + evaluate, failing **closed** (locked) on any deserialize error.
pub fn evaluate_content_json(clause_json: &str, lists: &[CuratorList]) -> EffectiveWebPolicy {
    match serde_json::from_str::<GrantContent>(clause_json) {
        Ok(c) => evaluate_content(&c, lists),
        Err(_) => EffectiveWebPolicy::locked(),
    }
}

/// Evaluate a parsed clause against verified curator lists.
pub fn evaluate_content(clause: &GrantContent, lists: &[CuratorList]) -> EffectiveWebPolicy {
    // Fail-closed precedence (matches spec §4.3): `paused` is checked BEFORE
    // `revoked`, so a contradictory clause with both set LOCKS (never opens).
    if clause.is_paused() {
        return EffectiveWebPolicy::locked();
    }
    if clause.is_revoked() {
        return EffectiveWebPolicy::unrestricted();
    }

    let safe_search = clause.safe_search_on();
    let youtube_restrict = clause.youtube_restrict.unwrap_or(YoutubeRestrict::Off);

    // REJECT rather than carry a domain that is not a plain DNS host (S11).
    // These end up in `DnsFilterPlan` and in Firefox `WebsiteFilter` patterns,
    // which are line-oriented sinks; a host with a newline in it is not a host.
    //
    // Dropping is safe in every direction, which is why it is a filter and not
    // an error: an unparseable entry could never have been enforced anyway, so
    // losing it costs nothing that was ever working, while carrying it risks
    // writing a rule nobody authored.
    let parent_allow: BTreeSet<String> = clause
        .parent_allow
        .iter()
        .filter_map(|d| parse_domain(d))
        .collect();
    let parent_deny: BTreeSet<String> = clause
        .parent_deny
        .iter()
        .filter_map(|d| parse_domain(d))
        .collect();
    let subscribed: BTreeSet<&str> = clause.curators.iter().map(|s| s.as_str()).collect();

    match clause.posture {
        Posture::Allowlist => {
            let quorum = clause.effective_quorum() as usize;
            // domain -> distinct curators that rated it kid-safe
            let mut votes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for l in lists {
                if !subscribed.contains(l.curator.as_str()) {
                    continue;
                }
                for e in &l.entries {
                    if let (Rating::KidSafe, Some(domain)) = (&e.rating, parse_domain(&e.domain)) {
                        votes.entry(domain).or_default().insert(l.curator.clone());
                    }
                }
            }
            let mut allow: BTreeSet<String> = votes
                .into_iter()
                .filter(|(_, curators)| curators.len() >= quorum)
                .map(|(d, _)| d)
                .collect();
            allow.extend(parent_allow.iter().cloned());
            for d in &parent_deny {
                allow.remove(d);
            }
            EffectiveWebPolicy {
                posture: Some(Posture::Allowlist),
                locked: false,
                allow_domains: allow,
                block_domains: BTreeSet::new(),
                block_categories: BTreeSet::new(),
                allow_exceptions: parent_allow.clone(),
                safe_search,
                youtube_restrict,
            }
        }
        Posture::Blocklist => {
            let mut block: BTreeSet<String> = BTreeSet::new();
            for l in lists {
                if !subscribed.contains(l.curator.as_str()) {
                    continue;
                }
                for e in &l.entries {
                    if let (Rating::Block, Some(domain)) = (&e.rating, parse_domain(&e.domain)) {
                        block.insert(domain);
                    }
                }
            }
            let categories: BTreeSet<String> = clause
                .block_categories
                .iter()
                .map(|c| c.to_ascii_lowercase())
                .collect();
            // parentAllow unblocks; parentDeny adds (parentDeny beats parentAllow).
            for d in &parent_allow {
                block.remove(d);
            }
            block.extend(parent_deny.iter().cloned());
            EffectiveWebPolicy {
                posture: Some(Posture::Blocklist),
                locked: false,
                allow_domains: BTreeSet::new(),
                block_domains: block,
                block_categories: categories,
                allow_exceptions: parent_allow.clone(),
                safe_search,
                youtube_restrict,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::{AgeTier, Posture};

    fn base_clause() -> GrantContent {
        GrantContent {
            v: 1,
            tz: None,
            posture: Posture::Allowlist,
            age_tier: AgeTier::Young,
            curators: vec![],
            quorum_n: None,
            block_categories: vec![],
            safe_search: None,
            youtube_restrict: None,
            parent_allow: vec![],
            parent_deny: vec![],
            paused: None,
            revoked: None,
            issued_at: 1,
        }
    }

    #[test]
    fn paused_locks_all_web() {
        let mut c = base_clause();
        c.paused = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(p.locked);
        assert_eq!(p.posture, None);
    }

    #[test]
    fn revoked_is_unrestricted() {
        let mut c = base_clause();
        c.revoked = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(!p.locked);
        assert_eq!(p.posture, None);
        assert!(p.allow_domains.is_empty());
    }

    #[test]
    fn paused_beats_revoked_fails_closed() {
        // A contradictory clause (both set) must LOCK, not open (spec §4.3).
        let mut c = base_clause();
        c.paused = Some(true);
        c.revoked = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(p.locked, "paused must win over revoked (fail-closed)");
    }

    #[test]
    fn blocklist_surfaces_parent_allow_as_exceptions() {
        // parentAllow can't unblock a CATEGORY at evaluate time (categories
        // expand at the DNS layer), so it's surfaced for the DNS enactor.
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.parent_allow = vec!["https://allowed.example/".into()];
        let p = evaluate_content(&c, &[]);
        assert!(p.allow_exceptions.contains("allowed.example"));
    }

    #[test]
    fn unparseable_json_fails_closed() {
        let p = evaluate_content_json("{ not json", &[]);
        assert!(p.locked);
    }

    use crate::curator::{CuratorEntry, CuratorList, Rating};

    fn list(curator: &str, entries: &[(&str, Rating)]) -> CuratorList {
        CuratorList {
            curator: curator.to_string(),
            entries: entries
                .iter()
                .map(|(d, r)| CuratorEntry {
                    domain: d.to_string(),
                    rating: r.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn allowlist_admits_domain_only_at_quorum() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into(), "bb".into()];
        c.quorum_n = Some(2);
        let lists = vec![
            list(
                "aa",
                &[
                    ("kids.example", Rating::KidSafe),
                    ("solo.example", Rating::KidSafe),
                ],
            ),
            list("bb", &[("kids.example", Rating::KidSafe)]),
        ];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("kids.example")); // 2 curators → admitted
        assert!(!p.allow_domains.contains("solo.example")); // 1 curator → below quorum
    }

    #[test]
    fn allowlist_ignores_unsubscribed_curators() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into()];
        c.quorum_n = Some(1);
        let lists = vec![list("zz", &[("evil.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.is_empty()); // zz not subscribed
    }

    #[test]
    fn allowlist_parent_overrides_win() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into()];
        c.quorum_n = Some(1);
        c.parent_allow = vec!["https://parent.example/".into()];
        c.parent_deny = vec!["kids.example".into()];
        let lists = vec![list("aa", &[("kids.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("parent.example")); // parentAllow added
        assert!(!p.allow_domains.contains("kids.example")); // parentDeny beats curator
    }

    #[test]
    fn allowlist_parent_deny_beats_dns_root_dot_variant() {
        // A curator admits the DNS-equivalent trailing-dot variant. parentDeny
        // of the bare host MUST still remove it (parentDeny is always
        // authoritative) — otherwise the child reaches a parent-denied site.
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into()];
        c.quorum_n = Some(1);
        c.parent_deny = vec!["evil.example".into()];
        let lists = vec![list("aa", &[("evil.example.", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(!p.allow_domains.contains("evil.example")); // parentDeny removed it
        assert!(!p.allow_domains.contains("evil.example.")); // variant normalized away
    }

    #[test]
    fn blocklist_aggregates_curator_blocks_and_categories() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.curators = vec!["aa".into()];
        c.block_categories = vec!["Gambling".into(), "porn".into()];
        let lists = vec![list("aa", &[("bad.example", Rating::Block)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.block_domains.contains("bad.example"));
        // categories are lowercased, passed through (DNS layer expands them)
        assert!(p.block_categories.contains("gambling"));
        assert!(p.block_categories.contains("porn"));
    }

    #[test]
    fn blocklist_parent_overrides_win() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.curators = vec!["aa".into()];
        c.parent_allow = vec!["bad.example".into()]; // unblock a curator block
        c.parent_deny = vec!["https://extra.example/".into()]; // add a block
        let lists = vec![list("aa", &[("bad.example", Rating::Block)])];
        let p = evaluate_content(&c, &lists);
        assert!(!p.block_domains.contains("bad.example")); // parentAllow removed it
        assert!(p.block_domains.contains("extra.example")); // parentDeny added it
    }
}
