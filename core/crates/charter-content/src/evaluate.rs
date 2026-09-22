//! The pure content evaluator: `GrantContent` + verified curator lists →
//! `EffectiveWebPolicy`. No I/O, no clock. Deterministic (`BTreeSet`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::clause::{GrantContent, Posture, YoutubeRestrict};
use crate::curator::{normalize_curator, CuratorList, Rating};
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
    /// `parentAllow` / `parentDeny` entries that were dropped for not being a
    /// plain DNS host — including a raw Unicode / IDN host, since this build
    /// has no IDNA-to-ASCII conversion (S11, review 2026-09-21 02-G4). Surfaced
    /// rather than silently discarded so the guardian app can show "N entries
    /// could not be applied".
    pub rejected_domains: Vec<String>,
    /// Count of `curators` entries that were not a 64-hex-char pubkey and so
    /// could never match a subscribed list (S11, review 2026-09-21 02-G3).
    pub rejected_curators: usize,
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
            rejected_domains: Vec::new(),
            rejected_curators: 0,
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
            rejected_domains: Vec::new(),
            rejected_curators: 0,
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
    // A body version this build does not implement reads as UNREADABLE — the
    // same fail-CLOSED state an unparseable clause already gets (S11, review
    // 2026-09-21 02-G1). Checked before `paused`/`revoked`, so no future field
    // can reach a v1 arm.
    if !clause.is_supported_version() {
        return EffectiveWebPolicy::locked();
    }
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
    let mut rejected_domains: Vec<String> = Vec::new();
    let parent_allow: BTreeSet<String> = clause
        .parent_allow
        .iter()
        .filter_map(|d| match parse_domain(d) {
            Some(parsed) => Some(parsed),
            None => {
                rejected_domains.push(d.clone());
                None
            }
        })
        .collect();
    let parent_deny: BTreeSet<String> = clause
        .parent_deny
        .iter()
        .filter_map(|d| match parse_domain(d) {
            Some(parsed) => Some(parsed),
            None => {
                rejected_domains.push(d.clone());
                None
            }
        })
        .collect();
    // Normalise + drop malformed curator ids (S11, review 2026-09-21 02-G3):
    // a case mismatch or a garbage id must never silently narrow the child's
    // web access in allowlist posture with no diagnostic anywhere.
    let mut rejected_curators = 0usize;
    let subscribed: BTreeSet<String> = clause
        .curators
        .iter()
        .filter_map(|s| match normalize_curator(s) {
            Some(id) => Some(id),
            None => {
                rejected_curators += 1;
                None
            }
        })
        .collect();
    // "parentDeny beats parentAllow" has to hold on EVERY output channel. The
    // exceptions list used to be the raw parentAllow, so a domain in both
    // lists — allowed once, denied later: the obvious UI flow — was blocked
    // AND published as an exception, leaving the answer to whichever rule the
    // DNS applier happened to rank first.
    let allow_exceptions: BTreeSet<String> =
        parent_allow.difference(&parent_deny).cloned().collect();

    match clause.posture {
        Posture::Allowlist => {
            let quorum = clause.effective_quorum() as usize;
            // domain -> distinct curators that rated it kid-safe
            let mut votes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for l in lists {
                let curator_id = match normalize_curator(&l.curator) {
                    Some(id) => id,
                    None => continue,
                };
                if !subscribed.contains(&curator_id) {
                    continue;
                }
                for e in &l.entries {
                    if let (Rating::KidSafe, Some(domain)) = (&e.rating, parse_domain(&e.domain)) {
                        votes.entry(domain).or_default().insert(curator_id.clone());
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
                allow_exceptions: allow_exceptions.clone(),
                safe_search,
                youtube_restrict,
                rejected_domains,
                rejected_curators,
            }
        }
        Posture::Blocklist => {
            let mut block: BTreeSet<String> = BTreeSet::new();
            for l in lists {
                let curator_id = match normalize_curator(&l.curator) {
                    Some(id) => id,
                    None => continue,
                };
                if !subscribed.contains(&curator_id) {
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
                allow_exceptions: allow_exceptions.clone(),
                safe_search,
                youtube_restrict,
                rejected_domains,
                rejected_curators,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::{AgeTier, Posture};

    // 64-hex curator ids (the wire shape, `spec/contract.md:191`) — short
    // stand-ins like "aa" no longer pass `normalize_curator` (S11, review
    // 2026-09-21 02-G3), so tests need a real-shaped id.
    const CURATOR_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CURATOR_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const CURATOR_Z: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

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

    // S11 (review 2026-09-21, 02-G1): an unknown future `v` must read as
    // UNREADABLE (locked), not be silently evaluated as v1.
    #[test]
    fn an_unsupported_version_fails_closed() {
        let mut c = base_clause();
        c.v = crate::clause::CONTENT_VERSION + 1;
        // Even a wide-open shape (revoked) must still lock: the version gate
        // is checked before paused/revoked.
        c.revoked = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(p.locked, "an unsupported version must fail closed");
    }

    #[test]
    fn the_current_version_is_supported() {
        let c = base_clause();
        assert!(c.is_supported_version());
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
        c.curators = vec![CURATOR_A.into(), CURATOR_B.into()];
        c.quorum_n = Some(2);
        let lists = vec![
            list(
                CURATOR_A,
                &[
                    ("kids.example", Rating::KidSafe),
                    ("solo.example", Rating::KidSafe),
                ],
            ),
            list(CURATOR_B, &[("kids.example", Rating::KidSafe)]),
        ];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("kids.example")); // 2 curators → admitted
        assert!(!p.allow_domains.contains("solo.example")); // 1 curator → below quorum
    }

    #[test]
    fn allowlist_ignores_unsubscribed_curators() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec![CURATOR_A.into()];
        c.quorum_n = Some(1);
        let lists = vec![list(CURATOR_Z, &[("evil.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.is_empty()); // CURATOR_Z not subscribed
    }

    #[test]
    fn allowlist_parent_overrides_win() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec![CURATOR_A.into()];
        c.quorum_n = Some(1);
        c.parent_allow = vec!["https://parent.example/".into()];
        c.parent_deny = vec!["kids.example".into()];
        let lists = vec![list(CURATOR_A, &[("kids.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("parent.example")); // parentAllow added
        assert!(!p.allow_domains.contains("kids.example")); // parentDeny beats curator
    }

    #[test]
    fn curator_ids_are_case_folded_and_malformed_ones_are_counted() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        // Upper-case on the clause side, lower-case on the verified list side —
        // must still match. One malformed id alongside a valid one is counted.
        c.curators = vec![CURATOR_A.to_ascii_uppercase(), "not-hex".into()];
        c.quorum_n = Some(1);
        let lists = vec![list(CURATOR_A, &[("kids.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("kids.example"));
        assert_eq!(p.rejected_curators, 1);
    }

    /// A domain allowed once and denied later sits in BOTH lists. It must not
    /// leave as an exception on either posture — that is the channel the DNS
    /// plan documents as "parent allows that override category blocks".
    #[test]
    fn a_domain_in_both_lists_is_never_published_as_an_exception() {
        for posture in [Posture::Allowlist, Posture::Blocklist] {
            let mut c = base_clause();
            c.posture = posture;
            c.parent_allow = vec!["both.example".into(), "kept.example".into()];
            c.parent_deny = vec!["both.example".into()];
            let p = evaluate_content(&c, &[]);
            assert!(!p.allow_exceptions.contains("both.example"));
            assert!(p.allow_exceptions.contains("kept.example"));
        }
    }

    #[test]
    fn allowlist_parent_deny_beats_dns_root_dot_variant() {
        // A curator admits the DNS-equivalent trailing-dot variant. parentDeny
        // of the bare host MUST still remove it (parentDeny is always
        // authoritative) — otherwise the child reaches a parent-denied site.
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec![CURATOR_A.into()];
        c.quorum_n = Some(1);
        c.parent_deny = vec!["evil.example".into()];
        let lists = vec![list(CURATOR_A, &[("evil.example.", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(!p.allow_domains.contains("evil.example")); // parentDeny removed it
        assert!(!p.allow_domains.contains("evil.example.")); // variant normalized away
    }

    #[test]
    fn blocklist_aggregates_curator_blocks_and_categories() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.curators = vec![CURATOR_A.into()];
        c.block_categories = vec!["Gambling".into(), "porn".into()];
        let lists = vec![list(CURATOR_A, &[("bad.example", Rating::Block)])];
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
        c.curators = vec![CURATOR_A.into()];
        c.parent_allow = vec!["bad.example".into()]; // unblock a curator block
        c.parent_deny = vec!["https://extra.example/".into()]; // add a block
        let lists = vec![list(CURATOR_A, &[("bad.example", Rating::Block)])];
        let p = evaluate_content(&c, &lists);
        assert!(!p.block_domains.contains("bad.example")); // parentAllow removed it
        assert!(p.block_domains.contains("extra.example")); // parentDeny added it
    }

    // S11 (review 2026-09-21, 02-G4): an IDN / raw-Unicode host in
    // parentAllow/parentDeny is dropped (no IDNA in this build) but must be
    // SURFACED, not silently lost.
    #[test]
    fn a_unicode_domain_is_dropped_but_surfaced_in_rejected() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.parent_deny = vec!["bücher.example".into(), "garbage domain!!".into()];
        let p = evaluate_content(&c, &[]);
        assert!(!p.block_domains.iter().any(|d| d.contains("bücher")));
        assert!(p.rejected_domains.contains(&"bücher.example".to_string()));
        assert!(p.rejected_domains.contains(&"garbage domain!!".to_string()));
    }

    #[test]
    fn a_valid_domain_is_not_surfaced_as_rejected() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.parent_deny = vec!["example.org".into()];
        let p = evaluate_content(&c, &[]);
        assert!(p.rejected_domains.is_empty());
    }
}
