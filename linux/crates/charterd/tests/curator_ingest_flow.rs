//! End-to-end (headless, mock) curator-list ingest: clause -> fetch -> cache ->
//! evaluate -> materialize. Proves quorum admission reaches the Firefox + DNS
//! artifacts and that an unsubscribed curator can never inject a domain.

#![cfg(feature = "mock")]

use charter_content::{CuratorEntry, CuratorList, Rating};
use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_sys::persistence::ClauseStore;
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::FetchedCuratorList;

use charterd::curator_sync::refresh_curator_lists;
use charterd::transport_facade::MockTransport;
use charterd::web_content::{WebContentEnforcer, WebReconcile};

fn pk(b: u8) -> PubKey {
    PubKey::from_bytes([b; 32])
}

fn fetched(curator: PubKey, domain: &str, rating: Rating) -> FetchedCuratorList {
    FetchedCuratorList {
        curator,
        list_id: "main".into(),
        created_at: 5,
        list: CuratorList {
            curator: curator.to_hex(),
            entries: vec![CuratorEntry {
                domain: domain.into(),
                rating,
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
async fn quorum_admits_domain_end_to_end() {
    let sys = MockSystem::new(1000);
    let c1 = pk(0xc1);
    let c2 = pk(0xc2);
    let clause = format!(
        r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}","{}"],"quorumN":2,"issuedAt":1}}"#,
        c1.to_hex(),
        c2.to_hex()
    );
    put_content(&sys, &clause);

    let transport = MockTransport::new(pk(0x11), pk(0x22));
    transport.deliver_curator_list(fetched(c1, "kids.example", Rating::KidSafe));
    transport.deliver_curator_list(fetched(c2, "kids.example", Rating::KidSafe));

    refresh_curator_lists(&sys, &transport).await;

    let mut enforcer = WebContentEnforcer::new();
    assert_eq!(
        enforcer.reconcile(&sys).await,
        WebReconcile::Enacted { locked: false }
    );

    let pol = sys.web_policy().last_policies().unwrap();
    assert!(
        pol.contains("kids.example"),
        "quorum domain must be in the Firefox allowlist"
    );
    let plan = sys.dns_filter().last_plan().unwrap();
    assert!(plan.contains("allowlist"));
}

#[tokio::test]
async fn unsubscribed_curator_cannot_inject_domain() {
    let sys = MockSystem::new(1000);
    let c1 = pk(0xc1);
    let rogue = pk(0xee); // not in curators[]
    let clause = format!(
        r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}"],"quorumN":1,"issuedAt":1}}"#,
        c1.to_hex()
    );
    put_content(&sys, &clause);

    let transport = MockTransport::new(pk(0x11), pk(0x22));
    transport.deliver_curator_list(fetched(c1, "kids.example", Rating::KidSafe));
    transport.deliver_curator_list(fetched(rogue, "adult.example", Rating::KidSafe));

    refresh_curator_lists(&sys, &transport).await;
    let mut enforcer = WebContentEnforcer::new();
    enforcer.reconcile(&sys).await;

    let pol = sys.web_policy().last_policies().unwrap();
    assert!(pol.contains("kids.example"));
    assert!(
        !pol.contains("adult.example"),
        "the evaluator filters by subscribed curators — a rogue list never admits a domain"
    );
}
