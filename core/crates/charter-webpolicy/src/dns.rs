//! Render an `EffectiveWebPolicy` into a structured DNS-layer plan for the
//! `dns_filter` enactor to apply to AdGuard Home (+ resolv.conf pin + firewall).
//!
//! Pure data, no I/O. Forced-SafeSearch / YouTube-restricted-mode are done here
//! (the DNS layer) via CNAME rewrites — Firefox has no SafeSearch policy.

use charter_content::{EffectiveWebPolicy, Posture, YoutubeRestrict};

/// A DNS CNAME rewrite (`host` → `answer`), used to force SafeSearch / restricted modes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRewrite {
    pub host: String,
    pub answer: String,
}

/// The resolve posture the DNS engine enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DnsMode {
    /// Default-deny: only `allow_domains` resolve.
    Allowlist,
    /// Default-allow minus blocks / categories.
    Blocklist,
    /// No DNS content constraint (revoked).
    Unrestricted,
    /// Everything blocked (paused / fail-closed).
    Locked,
}

/// The structured plan the `dns_filter` enactor applies. Pure — the enactor
/// translates it to AdGuard Home config + resolv.conf pin + firewall rules.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsFilterPlan {
    pub mode: DnsMode,
    pub allow_domains: Vec<String>,
    pub block_domains: Vec<String>,
    pub block_categories: Vec<String>,
    /// Parent allows that override category blocks (categories expand here).
    pub allow_exceptions: Vec<String>,
    pub safe_search: bool,
    pub youtube_restrict: YoutubeRestrict,
    pub rewrites: Vec<DnsRewrite>,
}

fn rewrite(host: &str, answer: &str) -> DnsRewrite {
    DnsRewrite {
        host: host.to_string(),
        answer: answer.to_string(),
    }
}

/// Forced-SafeSearch + YouTube-restricted CNAME rewrites (stable, research-sourced).
/// Never CNAME bare `youtube.com` / `youtu.be` — only the canonical service hosts.
fn safesearch_rewrites(youtube: YoutubeRestrict) -> Vec<DnsRewrite> {
    let mut r = vec![
        rewrite("www.google.com", "forcesafesearch.google.com"),
        rewrite("www.bing.com", "strict.bing.com"),
        rewrite("duckduckgo.com", "safe.duckduckgo.com"),
    ];
    let yt_target = match youtube {
        YoutubeRestrict::Off => None,
        YoutubeRestrict::Moderate => Some("restrictmoderate.youtube.com"),
        YoutubeRestrict::Strict => Some("restrict.youtube.com"),
    };
    if let Some(target) = yt_target {
        for host in [
            "www.youtube.com",
            "m.youtube.com",
            "youtubei.googleapis.com",
            "youtube.googleapis.com",
            "www.youtube-nocookie.com",
        ] {
            r.push(rewrite(host, target));
        }
    }
    r
}

/// Render the DNS-layer plan for an effective web policy.
pub fn render_dns_filter(policy: &EffectiveWebPolicy) -> DnsFilterPlan {
    let mode = if policy.locked {
        DnsMode::Locked
    } else {
        match policy.posture {
            Some(Posture::Allowlist) => DnsMode::Allowlist,
            Some(Posture::Blocklist) => DnsMode::Blocklist,
            None => DnsMode::Unrestricted,
        }
    };

    // Only force SafeSearch when actively filtering (not locked, not revoked).
    let rewrites = if policy.safe_search && !policy.locked && policy.posture.is_some() {
        safesearch_rewrites(policy.youtube_restrict)
    } else {
        Vec::new()
    };

    let mut block_domains: Vec<String> = policy.block_domains.iter().cloned().collect();
    // Refuse the DoH canary so browsers disable auto-DoH and use the system
    // (Charter-filtered) resolver. Harmless on Linux (charterd already blocks
    // it via network.trr.mode=5); load-bearing on the phone.
    if !block_domains.iter().any(|d| d == "use-application-dns.net") {
        block_domains.push("use-application-dns.net".to_string());
    }

    DnsFilterPlan {
        mode,
        allow_domains: policy.allow_domains.iter().cloned().collect(),
        block_domains,
        block_categories: policy.block_categories.iter().cloned().collect(),
        allow_exceptions: policy.allow_exceptions.iter().cloned().collect(),
        safe_search: policy.safe_search,
        youtube_restrict: policy.youtube_restrict,
        rewrites,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{evaluate_content, AgeTier, GrantContent, Posture, YoutubeRestrict};

    fn clause(posture: Posture) -> GrantContent {
        GrantContent {
            v: 1,
            tz: None,
            posture,
            age_tier: AgeTier::Older,
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

    fn hosts(rw: &[DnsRewrite]) -> Vec<&str> {
        rw.iter().map(|r| r.host.as_str()).collect()
    }

    #[test]
    fn allowlist_mode_carries_allow_domains() {
        let mut c = clause(Posture::Allowlist);
        c.parent_allow = vec!["kids.example".into()];
        let plan = render_dns_filter(&evaluate_content(&c, &[]));
        assert_eq!(plan.mode, DnsMode::Allowlist);
        assert!(plan.allow_domains.contains(&"kids.example".to_string()));
    }

    #[test]
    fn blocklist_mode_carries_blocks_categories_and_exceptions() {
        let mut c = clause(Posture::Blocklist);
        c.parent_deny = vec!["bad.example".into()];
        c.block_categories = vec!["gambling".into()];
        c.parent_allow = vec!["ok.example".into()];
        let plan = render_dns_filter(&evaluate_content(&c, &[]));
        assert_eq!(plan.mode, DnsMode::Blocklist);
        assert!(plan.block_domains.contains(&"bad.example".to_string()));
        assert!(plan.block_categories.contains(&"gambling".to_string()));
        assert!(plan.allow_exceptions.contains(&"ok.example".to_string()));
    }

    #[test]
    fn safesearch_on_by_default_emits_google_bing_ddg() {
        let plan = render_dns_filter(&evaluate_content(&clause(Posture::Blocklist), &[]));
        assert!(plan.safe_search);
        let h = hosts(&plan.rewrites);
        assert!(h.contains(&"www.google.com"));
        assert!(h.contains(&"www.bing.com"));
        assert!(h.contains(&"duckduckgo.com"));
    }

    #[test]
    fn youtube_restrict_levels_map_to_correct_cname() {
        let mut c = clause(Posture::Blocklist);
        c.youtube_restrict = Some(YoutubeRestrict::Off);
        let off = render_dns_filter(&evaluate_content(&c, &[]));
        assert!(!hosts(&off.rewrites).contains(&"www.youtube.com"));

        c.youtube_restrict = Some(YoutubeRestrict::Moderate);
        let mod_ = render_dns_filter(&evaluate_content(&c, &[]));
        let yt = mod_
            .rewrites
            .iter()
            .find(|r| r.host == "www.youtube.com")
            .unwrap();
        assert_eq!(yt.answer, "restrictmoderate.youtube.com");

        c.youtube_restrict = Some(YoutubeRestrict::Strict);
        let strict = render_dns_filter(&evaluate_content(&c, &[]));
        let yt = strict
            .rewrites
            .iter()
            .find(|r| r.host == "www.youtube.com")
            .unwrap();
        assert_eq!(yt.answer, "restrict.youtube.com");
    }

    #[test]
    fn locked_mode_blocks_with_no_rewrites() {
        let mut c = clause(Posture::Allowlist);
        c.paused = Some(true);
        let plan = render_dns_filter(&evaluate_content(&c, &[]));
        assert_eq!(plan.mode, DnsMode::Locked);
        assert!(plan.rewrites.is_empty());
    }

    #[test]
    fn revoked_is_unrestricted_no_rewrites() {
        let mut c = clause(Posture::Blocklist);
        c.revoked = Some(true);
        let plan = render_dns_filter(&evaluate_content(&c, &[]));
        assert_eq!(plan.mode, DnsMode::Unrestricted);
        assert!(!plan.safe_search);
        assert!(plan.rewrites.is_empty());
    }

    #[test]
    fn doh_canary_always_blocked() {
        let plan = render_dns_filter(&evaluate_content(&clause(Posture::Blocklist), &[]));
        assert!(plan
            .block_domains
            .iter()
            .any(|d| d == "use-application-dns.net"));
    }
}
