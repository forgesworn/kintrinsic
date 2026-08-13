//! Curator web-list ingest: a subscribed curator's signed kind-30100 list is
//! fetched + verified; a hostile relay that ignores the author filter or serves
//! a bad signature is rejected; replaceable (d-keyed) lists keep the newest.

#![cfg(feature = "mock")]

use async_trait::async_trait;

use charter_primitives::{kinds, NostrEvent, PubKey, Sig};
use charter_sys::relay::{
    Filter, MockRelayTransport, PublishOutcome, RelayIoError, RelayTransport, RelayUrl,
};
use charter_sys::signer::{MachineSigner, SeedSigner};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::sign_event;

fn machine_sk() -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = 0x22;
    s
}

fn gpk() -> PubKey {
    PubKey::from_bytes([0x11; 32])
}

/// Build a signed kind-30100 curator list event.
fn curator_event(
    signer: &SeedSigner,
    d: &str,
    created_at: u64,
    entries: &[(&str, &str)],
) -> NostrEvent {
    let mut tags = vec![
        vec!["d".to_string(), d.to_string()],
        vec!["L".to_string(), kinds::CURATOR_WEB_NAMESPACE.to_string()],
    ];
    for (domain, rating) in entries {
        tags.push(vec![
            "r".to_string(),
            domain.to_string(),
            rating.to_string(),
        ]);
    }
    sign_event(
        signer,
        kinds::CHARTER_CURATOR_WEB_LIST,
        created_at,
        tags,
        String::new(),
    )
}

fn transport<R: RelayTransport>(relay: R) -> CharterTransport<R, ScriptedEntropy> {
    CharterTransport::new(
        relay,
        ScriptedEntropy::new(7),
        machine_sk(),
        gpk(),
        vec!["wss://r".into()],
    )
}

/// A relay that returns every injected event regardless of the filter —
/// models a hostile relay that ignores the author subscription.
struct HostileRelay {
    events: Vec<NostrEvent>,
}

#[async_trait]
impl RelayTransport for HostileRelay {
    async fn publish(&self, _r: &[RelayUrl], _ev: NostrEvent) -> Vec<(RelayUrl, PublishOutcome)> {
        Vec::new()
    }
    async fn query(&self, _r: &[RelayUrl], _f: Filter) -> Result<Vec<NostrEvent>, RelayIoError> {
        Ok(self.events.clone())
    }
}

#[tokio::test]
async fn fetches_and_verifies_subscribed_curator_list() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(
        &curator,
        "main",
        5,
        &[("kids.example", "kid-safe")],
    ));

    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].curator, curator_pk);
    assert_eq!(got[0].list.curator, curator_pk.to_hex());
    assert_eq!(got[0].list.entries[0].domain, "kids.example");
}

#[tokio::test]
async fn empty_subscription_fetches_nothing() {
    let curator = SeedSigner::from_seed(0xC1);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(
        &curator,
        "main",
        5,
        &[("kids.example", "kid-safe")],
    ));
    let t = transport(relay);
    assert!(t.poll_curator_lists(&[], 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn hostile_relay_cannot_inject_foreign_or_forged_lists() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let rogue = SeedSigner::from_seed(0xEE); // not subscribed

    let valid = curator_event(&curator, "main", 5, &[("kids.example", "kid-safe")]);
    let foreign = curator_event(&rogue, "main", 5, &[("adult.example", "kid-safe")]);
    let mut forged = curator_event(&curator, "x", 6, &[("evil.example", "kid-safe")]);
    forged.sig = Sig::from_bytes([0u8; 64]); // tampered signature

    let relay = HostileRelay {
        events: vec![valid, foreign, forged],
    };
    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();

    assert_eq!(
        got.len(),
        1,
        "only the genuine subscribed+signed list survives"
    );
    assert_eq!(got[0].curator, curator_pk);
    assert_eq!(got[0].list.entries[0].domain, "kids.example");
}

#[tokio::test]
async fn replaceable_keeps_newest_per_list_id() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(
        &curator,
        "main",
        5,
        &[("old.example", "kid-safe")],
    ));
    relay.inject(curator_event(
        &curator,
        "main",
        9,
        &[("new.example", "kid-safe")],
    ));

    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();
    assert_eq!(got.len(), 1, "same (curator, d) collapses to the newest");
    assert_eq!(got[0].created_at, 9);
    assert_eq!(got[0].list.entries[0].domain, "new.example");
}
