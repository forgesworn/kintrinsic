//! The transport seam the broker depends on. The daemon spine is built against
//! a **mock** facade so a Phase-2 transport-crypto stall never blocks it; the
//! `CharterTransportAdapter` bridges the real `charter-transport`.

use std::sync::Mutex;

use async_trait::async_trait;

use charter_primitives::{NostrEvent, PubKey};
use charter_transport::{FetchedCuratorList, ReceivedClause, ReceivedGrant};

/// Delivery seam: publish requests/audit, poll for grants/clauses. Returns
/// **unverified** events — the broker authenticates them.
#[async_trait]
pub trait TransportFacade: Send + Sync {
    async fn publish_request(&self, request_json: &str, now: u64);
    async fn poll_grants(&self, since: u64, now: u64) -> Vec<ReceivedGrant>;
    async fn poll_clauses(&self, since: u64, now: u64) -> Vec<ReceivedClause>;
    /// Delivered USAGE_SYNC events (kind 31115), unverified — the broker runs
    /// `verify_usage_sync`. `(event, seal_author)`, like `poll_releases`.
    async fn poll_usage_syncs(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)>;
    /// Delivered RELEASE events (kind 31116) — the guardian-signed unpair.
    /// Unverified `(event, seal_author)`; the broker runs `verify_release`.
    async fn poll_releases(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)>;
    async fn poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Vec<FetchedCuratorList>;
    async fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64);
    fn pinned_guardian(&self) -> PubKey;
    fn machine_pubkey(&self) -> PubKey;
}

/// An in-memory mock transport: scriptable grant/clause delivery + recorders.
pub struct MockTransport {
    guardian: PubKey,
    machine: PubKey,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    grants: Vec<ReceivedGrant>,
    clauses: Vec<ReceivedClause>,
    usage_syncs: Vec<(NostrEvent, PubKey)>,
    releases: Vec<(NostrEvent, PubKey)>,
    curator_lists: Vec<FetchedCuratorList>,
    published: Vec<String>,
    audits: Vec<Vec<Vec<String>>>,
}

impl MockTransport {
    pub fn new(guardian: PubKey, machine: PubKey) -> Self {
        MockTransport {
            guardian,
            machine,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Queue a grant for delivery, sealed by `seal_author` (the guardian, or an
    /// attacker for hostile-relay tests).
    pub fn deliver_grant(&self, grant: NostrEvent, seal_author: PubKey) {
        self.inner
            .lock()
            .expect("lock")
            .grants
            .push(ReceivedGrant { grant, seal_author });
    }

    /// Queue a clause for delivery.
    pub fn deliver_clause(&self, clause: NostrEvent, seal_author: PubKey) {
        self.inner
            .lock()
            .expect("lock")
            .clauses
            .push(ReceivedClause {
                clause,
                seal_author,
            });
    }

    /// Queue a USAGE_SYNC event for delivery, sealed by `seal_author`.
    pub fn deliver_usage_sync(&self, event: NostrEvent, seal_author: PubKey) {
        self.inner
            .lock()
            .expect("lock")
            .usage_syncs
            .push((event, seal_author));
    }

    /// Queue a RELEASE event for delivery, sealed by `seal_author` (the
    /// guardian, or an attacker for hostile-relay tests).
    pub fn deliver_release(&self, event: NostrEvent, seal_author: PubKey) {
        self.inner
            .lock()
            .expect("lock")
            .releases
            .push((event, seal_author));
    }

    /// Queue a (already verified) curator list for delivery.
    pub fn deliver_curator_list(&self, list: FetchedCuratorList) {
        self.inner.lock().expect("lock").curator_lists.push(list);
    }

    /// All request payload JSONs published so far.
    pub fn published(&self) -> Vec<String> {
        self.inner.lock().expect("lock").published.clone()
    }

    /// All audit tag-sets emitted so far.
    pub fn audits(&self) -> Vec<Vec<Vec<String>>> {
        self.inner.lock().expect("lock").audits.clone()
    }
}

#[async_trait]
impl TransportFacade for MockTransport {
    async fn publish_request(&self, request_json: &str, _now: u64) {
        self.inner
            .lock()
            .expect("lock")
            .published
            .push(request_json.to_string());
    }

    async fn poll_grants(&self, _since: u64, _now: u64) -> Vec<ReceivedGrant> {
        std::mem::take(&mut self.inner.lock().expect("lock").grants)
    }

    async fn poll_clauses(&self, _since: u64, _now: u64) -> Vec<ReceivedClause> {
        std::mem::take(&mut self.inner.lock().expect("lock").clauses)
    }

    async fn poll_usage_syncs(&self, _since: u64, _now: u64) -> Vec<(NostrEvent, PubKey)> {
        std::mem::take(&mut self.inner.lock().expect("lock").usage_syncs)
    }

    async fn poll_releases(&self, _since: u64, _now: u64) -> Vec<(NostrEvent, PubKey)> {
        std::mem::take(&mut self.inner.lock().expect("lock").releases)
    }

    async fn poll_curator_lists(
        &self,
        _curators: &[PubKey],
        _since: u64,
    ) -> Vec<FetchedCuratorList> {
        std::mem::take(&mut self.inner.lock().expect("lock").curator_lists)
    }

    async fn emit_audit(&self, tags: Vec<Vec<String>>, _now: u64) {
        self.inner.lock().expect("lock").audits.push(tags);
    }

    fn pinned_guardian(&self) -> PubKey {
        self.guardian
    }

    fn machine_pubkey(&self) -> PubKey {
        self.machine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{CuratorEntry, CuratorList, Rating};

    fn pk(b: u8) -> PubKey {
        PubKey::from_bytes([b; 32])
    }

    #[tokio::test]
    async fn mock_delivers_then_drains_curator_lists() {
        let t = MockTransport::new(pk(0x11), pk(0x22));
        t.deliver_curator_list(FetchedCuratorList {
            curator: pk(0xaa),
            list_id: "main".into(),
            created_at: 5,
            list: CuratorList {
                curator: pk(0xaa).to_hex(),
                entries: vec![CuratorEntry {
                    domain: "kids.example".into(),
                    rating: Rating::KidSafe,
                }],
            },
        });
        let got = t.poll_curator_lists(&[pk(0xaa)], 0).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].list.entries[0].domain, "kids.example");
        assert!(
            t.poll_curator_lists(&[pk(0xaa)], 0).await.is_empty(),
            "drained"
        );
    }
}
