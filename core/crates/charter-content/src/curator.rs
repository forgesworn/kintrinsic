//! In-memory, already-signature-verified curator list. The Nostr wire parsing
//! that produces these lives in `charter-transport`. The serde representation
//! here is the *cache* format (variant names), NOT the wire token format.

use serde::{Deserialize, Serialize};

/// A curator's rating of a domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rating {
    KidSafe,
    Block,
    Category(String),
}

/// One `(domain, rating)` entry in a curator's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratorEntry {
    pub domain: String,
    pub rating: Rating,
}

/// A signed curator list, keyed by the curator's pubkey (64-hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratorList {
    pub curator: String,
    pub entries: Vec<CuratorEntry>,
}

/// Canonicalise a curator pubkey for comparison: lowercase hex, exactly 64
/// characters. `None` for anything else (S11, review 2026-09-21 02-G3) —
/// `spec/contract.md:191` specifies "Curator pubkeys (hex)", so there is a
/// canonical form, and comparing raw strings means a clause's `curators`
/// list and a verified `CuratorList.curator` written in different case never
/// match, which in allowlist posture is a silent, total web lockout.
pub fn normalize_curator(id: &str) -> Option<String> {
    if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(id.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curator_list_json_roundtrip() {
        let list = CuratorList {
            curator: "aa".into(),
            entries: vec![
                CuratorEntry {
                    domain: "kids.example".into(),
                    rating: Rating::KidSafe,
                },
                CuratorEntry {
                    domain: "bad.example".into(),
                    rating: Rating::Block,
                },
                CuratorEntry {
                    domain: "p.example".into(),
                    rating: Rating::Category("porn".into()),
                },
            ],
        };
        let json = serde_json::to_string(&list).expect("serialize");
        let back: CuratorList = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(list, back);
    }
}
