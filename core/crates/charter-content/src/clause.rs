//! The `content` clause data shape (device-enforced, frozen alongside
//! `spec/contract.md`) and its enums.

use serde::{Deserialize, Serialize};

/// Enforcement posture for a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Posture {
    Allowlist,
    Blocklist,
}

/// Coarse, parent-set policy selector. NOT derived from a date of birth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgeTier {
    Young,
    Older,
}

/// YouTube restricted-mode level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum YoutubeRestrict {
    #[default]
    Off,
    Moderate,
    Strict,
}

/// The runtime-canonical, **device-enforced** content clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantContent {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    pub posture: Posture,
    pub age_tier: AgeTier,
    #[serde(default)]
    pub curators: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quorum_n: Option<u32>,
    #[serde(default)]
    pub block_categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_restrict: Option<YoutubeRestrict>,
    #[serde(default)]
    pub parent_allow: Vec<String>,
    #[serde(default)]
    pub parent_deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<bool>,
    pub issued_at: u64,
}

impl GrantContent {
    /// Curators required to admit a domain into an allowlist. Default 2, floored at 1.
    pub fn effective_quorum(&self) -> u32 {
        self.quorum_n.unwrap_or(2).max(1)
    }

    /// SafeSearch defaults ON when unspecified.
    pub fn safe_search_on(&self) -> bool {
        self.safe_search.unwrap_or(true)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.unwrap_or(false)
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_content_roundtrips_canonical_json() {
        let json = r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"parentAllow":["https://example.com/"],"safeSearch":true,"youtubeRestrict":"moderate","issuedAt":1700000000}"#;
        let c: GrantContent = serde_json::from_str(json).unwrap();
        assert_eq!(c.posture, Posture::Allowlist);
        assert_eq!(c.age_tier, AgeTier::Young);
        assert_eq!(c.curators, vec!["aa".to_string(), "bb".to_string()]);
        assert_eq!(c.effective_quorum(), 2);
        assert!(c.safe_search_on());
        assert_eq!(c.youtube_restrict, Some(YoutubeRestrict::Moderate));
        assert!(!c.is_paused());
        assert!(!c.is_revoked());
        // Round-trips.
        let back: GrantContent = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn defaults_quorum_two_and_safesearch_on() {
        let json = r#"{"v":1,"posture":"blocklist","ageTier":"older","issuedAt":1}"#;
        let c: GrantContent = serde_json::from_str(json).unwrap();
        assert_eq!(c.effective_quorum(), 2);
        assert!(c.safe_search_on());
        assert!(c.curators.is_empty());
    }
}
