//! Curator-list ingest driver: read the cached `content` clause to learn the
//! subscribed curators, fetch their signed lists over the transport facade, and
//! write each to the rollback-protected cache. Fail-closed: a fetch failure (or
//! a clause without curators) writes nothing, so the last-known-good cache
//! stands and an allowlist never widens on failure.

use charter_content::GrantContent;
use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_sys::persistence::{ClauseStore, CuratorListStore};
use charter_sys::SystemLayer;

use crate::transport_facade::TransportFacade;

pub async fn refresh_curator_lists<S: SystemLayer, T: TransportFacade>(sys: &S, transport: &T) {
    let clause_json = match sys.clauses().get_clause(ClauseKind::Content.store_key()) {
        Ok(Some(j)) => j,
        _ => return,
    };
    let clause: GrantContent = match serde_json::from_str(&clause_json) {
        Ok(c) => c,
        Err(_) => return,
    };
    let curators: Vec<PubKey> = clause
        .curators
        .iter()
        .filter_map(|hex| PubKey::from_hex(hex).ok())
        .collect();
    if curators.is_empty() {
        return;
    }
    let fetched = transport.poll_curator_lists(&curators, 0).await;
    for f in fetched {
        if let Ok(json) = serde_json::to_string(&f.list) {
            let key = format!("{}:{}", f.curator.to_hex(), f.list_id);
            let _ = sys.curator_lists().put_list(&key, f.created_at, &json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{CuratorEntry, CuratorList, Rating};
    use charter_sys::MockSystem;
    use charter_transport::FetchedCuratorList;

    use crate::transport_facade::MockTransport;

    fn pk(b: u8) -> PubKey {
        PubKey::from_bytes([b; 32])
    }

    fn fetched(curator: PubKey, created_at: u64, domain: &str) -> FetchedCuratorList {
        FetchedCuratorList {
            curator,
            list_id: "main".into(),
            created_at,
            list: CuratorList {
                curator: curator.to_hex(),
                entries: vec![CuratorEntry {
                    domain: domain.into(),
                    rating: Rating::KidSafe,
                }],
            },
        }
    }

    fn put_content(sys: &MockSystem, json: &str) {
        sys.clauses()
            .put_clause(ClauseKind::Content.store_key(), 1, json)
            .unwrap();
    }

    #[tokio::test]
    async fn caches_lists_for_subscribed_curators() {
        let sys = MockSystem::new(1000);
        let c1 = pk(0xc1);
        let clause = format!(
            r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}"],"issuedAt":1}}"#,
            c1.to_hex()
        );
        put_content(&sys, &clause);
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        transport.deliver_curator_list(fetched(c1, 5, "kids.example"));

        refresh_curator_lists(&sys, &transport).await;

        let all = sys.curator_lists().all_lists().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].contains("kids.example"));
    }

    #[tokio::test]
    async fn clause_without_curators_is_noop() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","issuedAt":1}"#,
        );
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        transport.deliver_curator_list(fetched(pk(0xc1), 5, "kids.example"));

        refresh_curator_lists(&sys, &transport).await;
        assert!(sys.curator_lists().all_lists().unwrap().is_empty());
    }

    #[tokio::test]
    async fn absent_clause_is_noop() {
        let sys = MockSystem::new(1000);
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        refresh_curator_lists(&sys, &transport).await;
        assert!(sys.curator_lists().all_lists().unwrap().is_empty());
    }
}
