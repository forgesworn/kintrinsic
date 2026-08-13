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
