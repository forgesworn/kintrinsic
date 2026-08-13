//! Wire parsing for curator web-list events (kind 30100; NIP-51 addressable
//! `d`-tag + NIP-32 `L` namespace label). Turns a `NostrEvent` into the pure
//! `charter_content::CuratorList` the evaluator consumes. Signature
//! verification + relay fetch live in `transport.rs`; this module is pure.

use charter_content::{CuratorEntry, CuratorList, Rating};
use charter_primitives::{kinds, NostrEvent};

/// The `d`-tag value (NIP-51 addressable list id), or `""` if absent.
pub fn list_id(ev: &NostrEvent) -> String {
    ev.tag_value("d").unwrap_or("").to_string()
}

/// Parse a kind-30100 event into a `CuratorList`. Returns `None` when the event
/// is not a Charter curator web list (wrong kind, or missing the
/// `app.charter.web` namespace label). Malformed individual `r` entries are
/// skipped; the curator's pubkey is taken from the event author.
///
/// "Malformed" now includes the DOMAIN, not just the rating (S11, review
/// 2026-08-07). An `r`-tag host arrived here entirely unvalidated and flowed
/// into `DnsFilterPlan` and Firefox `WebsiteFilter` patterns — line-oriented
/// sinks — with control characters intact. Rejected at the door rather than
/// sanitised: a domain we had to edit to make safe is a domain whose meaning
/// we no longer know. One bad row is dropped; the rest of the curator's list
/// is still good.
pub fn parse_curator_list(ev: &NostrEvent) -> Option<CuratorList> {
    if ev.kind != kinds::CHARTER_CURATOR_WEB_LIST {
        return None;
    }
    if !ev.has_tag("L", kinds::CURATOR_WEB_NAMESPACE) {
        return None;
    }
    let entries = ev
        .tags
        .iter()
        .filter(|t| t.len() >= 3 && t[0] == "r")
        .filter_map(|t| {
            let domain = charter_content::domain::parse_domain(&t[1])?;
            parse_rating(&t[2]).map(|rating| CuratorEntry { domain, rating })
        })
        .collect();
    Some(CuratorList {
        curator: ev.pubkey.to_hex(),
        entries,
    })
}

fn parse_rating(token: &str) -> Option<Rating> {
    if token == kinds::CURATOR_RATING_KID_SAFE {
        Some(Rating::KidSafe)
    } else if token == kinds::CURATOR_RATING_BLOCK {
        Some(Rating::Block)
    } else {
        token
            .strip_prefix(kinds::CURATOR_RATING_CATEGORY_PREFIX)
            .filter(|k| !k.is_empty())
            .map(|k| Rating::Category(k.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::{EventId, PubKey, Sig};

    fn ev(kind: u16, tags: Vec<Vec<String>>) -> NostrEvent {
        NostrEvent {
            id: EventId::from_bytes([0; 32]),
            pubkey: PubKey::from_bytes([0xAB; 32]),
            created_at: 1,
            kind,
            tags,
            content: String::new(),
            sig: Sig::from_bytes([0; 64]),
        }
    }

    fn tag(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn ns() -> Vec<String> {
        tag(&["L", "app.charter.web"])
    }

    #[test]
    fn parses_ratings_and_curator_pubkey() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![
                tag(&["d", "main"]),
                ns(),
                tag(&["r", "kids.example", "kid-safe"]),
                tag(&["r", "bad.example", "block"]),
                tag(&["r", "p.example", "category:porn"]),
            ],
        );
        let list = parse_curator_list(&e).expect("parses");
        assert_eq!(list.curator, PubKey::from_bytes([0xAB; 32]).to_hex());
        assert_eq!(list.entries.len(), 3);
        assert_eq!(
            list.entries[0],
            CuratorEntry {
                domain: "kids.example".into(),
                rating: Rating::KidSafe
            }
        );
        assert_eq!(list.entries[1].rating, Rating::Block);
        assert_eq!(list.entries[2].rating, Rating::Category("porn".into()));
        assert_eq!(list_id(&e), "main");
    }

    #[test]
    fn rejects_wrong_kind() {
        let e = ev(kinds::CHARTER_DEVICE_CLAUSE, vec![ns()]);
        assert!(parse_curator_list(&e).is_none());
    }

    #[test]
    fn rejects_missing_namespace_label() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![tag(&["d", "main"]), tag(&["r", "kids.example", "kid-safe"])],
        );
        assert!(parse_curator_list(&e).is_none());
    }

    #[test]
    fn skips_malformed_entries() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![
                ns(),
                tag(&["r", "ok.example", "kid-safe"]),
                tag(&["r", "no-rating.example"]), // too short — skipped
                tag(&["r", "weird.example", "bogus"]), // unknown token — skipped
                tag(&["r", "empty.example", "category:"]), // empty key — skipped
            ],
        );
        let list = parse_curator_list(&e).expect("parses");
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].domain, "ok.example");
    }

    #[test]
    fn missing_d_tag_yields_empty_list_id() {
        let e = ev(kinds::CHARTER_CURATOR_WEB_LIST, vec![ns()]);
        assert_eq!(list_id(&e), "");
        assert!(parse_curator_list(&e).expect("parses").entries.is_empty());
    }
}
