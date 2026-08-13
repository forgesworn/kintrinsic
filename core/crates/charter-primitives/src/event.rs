//! The NIP-01 signed event shape — the single wire envelope shared by every
//! Charter device message (request, grant, clause, audit) and by every
//! gift-wrap layer (seal, wrap).

use serde::{Deserialize, Serialize};

use crate::ids::{EventId, PubKey, Sig};

/// A NIP-01 event. Field order and names match the Nostr wire format so a
/// canonical re-serialization byte-matches what `nostr-tools` produces.
///
/// `kind` is a `u16` (NIP-01 bounds kinds to `0..=65535`). `created_at` is
/// unsigned unix seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NostrEvent {
    /// 32-byte NIP-01 event id (sha256 of the canonical serialization).
    pub id: EventId,
    /// 32-byte x-only author public key.
    pub pubkey: PubKey,
    /// Unix seconds.
    pub created_at: u64,
    /// Event kind (`0..=65535`).
    pub kind: u16,
    /// Tag list; each tag is a list of strings, the first being the tag name.
    pub tags: Vec<Vec<String>>,
    /// Opaque content payload.
    pub content: String,
    /// 64-byte BIP-340 schnorr signature over `id`.
    pub sig: Sig,
}

impl NostrEvent {
    /// Returns true if any tag matches `[name, value]` (first two elements).
    pub fn has_tag(&self, name: &str, value: &str) -> bool {
        self.tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == name && t[1] == value)
    }

    /// First value of the first tag named `name`, if present.
    pub fn tag_value(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.len() >= 2 && t[0] == name)
            .map(|t| t[1].as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, PubKey, Sig};

    fn sample() -> NostrEvent {
        NostrEvent {
            id: EventId::from_hex(&"11".repeat(32)).unwrap(),
            pubkey: PubKey::from_hex(&"22".repeat(32)).unwrap(),
            created_at: 1_700_000_000,
            kind: 31112,
            tags: vec![vec!["t".into(), "charter-device".into()]],
            content: "hello".into(),
            sig: Sig::from_hex(&"33".repeat(64)).unwrap(),
        }
    }

    #[test]
    fn relay_event_serde_roundtrip() {
        let ev = sample();
        let json = serde_json::to_string(&ev).unwrap();
        let back: NostrEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn event_json_uses_hex_strings_and_numeric_kind() {
        let ev = sample();
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["kind"], 31112);
        assert_eq!(v["created_at"], 1_700_000_000u64);
        assert!(v["id"]
            .as_str()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tag_helpers() {
        let ev = sample();
        assert!(ev.has_tag("t", "charter-device"));
        assert!(!ev.has_tag("t", "other"));
        assert_eq!(ev.tag_value("t"), Some("charter-device"));
        assert_eq!(ev.tag_value("missing"), None);
    }
}
