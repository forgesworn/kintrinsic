//! The transport seam the broker depends on. The daemon spine is built against
//! a **mock** facade so a Phase-2 transport-crypto stall never blocks it; the
//! [`CharterTransportAdapter`] bridges the real `charter-transport`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_sys::relay::{PublishOutcome, RelayIoError, RelayTransport, RelayUrl};
use charter_transport::{
    CharterTransport, Entropy, FetchedCuratorList, ReceivedClause, ReceivedGrant,
};

/// A guardian wrap that arrived over the local link, unwrapped by the same
/// code as its relay copy and sorted by inner kind the way the `poll_*`
/// methods sort relay deliveries. Every inner event is still **unverified**:
/// the broker authenticates it exactly as it would a polled one.
#[derive(Debug, Clone)]
pub enum LocalInbound {
    Clause(ReceivedClause),
    Grant(ReceivedGrant),
    /// A RELEASE `(event, seal_author)`, as `poll_releases` returns it.
    Release(NostrEvent, PubKey),
    /// A USAGE_SYNC event (its seal author is unused, as on the poll path).
    UsageSync(NostrEvent),
    /// Anything else that unwrapped: not acted on over the local link.
    Other {
        inner_kind: u16,
    },
}

/// Sort an unwrapped inner event into a [`LocalInbound`] by kind, matching
/// the `poll_grants` / `poll_clauses` / `poll_releases` / `poll_usage_syncs`
/// split. A facade over a `CharterTransport` implements
/// [`TransportFacade::unwrap_local`] as
/// `t.unwrap_inbound(wrap, now).map(|(ev, author)| classify_inbound(ev, author))`.
pub fn classify_inbound(event: NostrEvent, seal_author: PubKey) -> LocalInbound {
    match event.kind {
        kinds::CHARTER_DEVICE_CLAUSE => LocalInbound::Clause(ReceivedClause {
            clause: event,
            seal_author,
        }),
        kinds::CHARTER_DEVICE_GRANT => LocalInbound::Grant(ReceivedGrant {
            grant: event,
            seal_author,
        }),
        kinds::CHARTER_DEVICE_RELEASE => LocalInbound::Release(event, seal_author),
        kinds::CHARTER_DEVICE_USAGE_SYNC => LocalInbound::UsageSync(event),
        inner_kind => LocalInbound::Other { inner_kind },
    }
}

/// Relay reachability + publish-failure signal for the most recent round of
/// polling/publishing, read (and reset) once per `Broker::poll_once` call.
///
/// Exists because every `poll_*`/`emit_*` method above returns a bare `Vec`
/// (no `Result`) — a shape the Android JNI facade also implements, so it
/// cannot change without touching `android/`. `charter-sys` already
/// distinguishes "every relay unreachable" (`RelayIoError::Unreachable`) from
/// "reached, nothing new" (`Ok(vec![])`); a facade that has that information
/// (`RealTransportFacade`) surfaces it here instead of silently folding both
/// into an empty `Vec`, which is exactly the fail-open shape B4/G1's sibling
/// review items flagged elsewhere in this slice.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PollHealth {
    /// At least one poll this round could not reach ANY relay in the set.
    pub relays_unreachable: bool,
    /// Per-relay publish failures accumulated this round (each failed relay
    /// on a single publish counts once).
    pub publish_failed: u32,
}

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

    /// Read-and-reset [`PollHealth`] for the round just finished. Default:
    /// always healthy — the historic shape every existing facade (the
    /// Android JNI bridge, this module's `MockTransport`) had before this
    /// field existed. `RealTransportFacade` overrides it.
    fn poll_health(&self) -> PollHealth {
        PollHealth::default()
    }

    /// Unwrap one gift wrap that arrived over the local link, through the
    /// same NIP-59 code as relay polling, and sort it by inner kind (see
    /// [`classify_inbound`]). `None` when the wrap does not open for this
    /// machine. Synchronous: unwrapping is CPU only, never IO.
    ///
    /// Default `None`: a facade that cannot unwrap (the scripted
    /// [`MockTransport`], an older platform facade) refuses every local wrap,
    /// and its relay path is unaffected.
    fn unwrap_local(&self, _wrap: &NostrEvent, _now: u64) -> Option<LocalInbound> {
        None
    }
}

/// A [`TransportFacade`] over a real [`CharterTransport`]: every relay error
/// is folded into [`PollHealth`] (as `RealTransportFacade` does on Linux), and
/// local wraps are unwrapped by [`CharterTransport::unwrap_inbound`]. The
/// reference shape for platform facades, and what the spine's own
/// end-to-end tests run on (over `MockRelayTransport`).
pub struct CharterTransportAdapter<R: RelayTransport, E: Entropy> {
    inner: CharterTransport<R, E>,
    relays_unreachable: AtomicBool,
    publish_failed: AtomicU32,
}

impl<R: RelayTransport, E: Entropy> CharterTransportAdapter<R, E> {
    /// Wrap `inner`; its pinned guardian is the key the broker authenticates
    /// against.
    pub fn new(inner: CharterTransport<R, E>) -> Self {
        CharterTransportAdapter {
            inner,
            relays_unreachable: AtomicBool::new(false),
            publish_failed: AtomicU32::new(0),
        }
    }

    /// The wrapped transport (STATUS publishing, test inspection).
    pub fn inner(&self) -> &CharterTransport<R, E> {
        &self.inner
    }

    fn record_poll<T>(&self, r: Result<Vec<T>, RelayIoError>) -> Vec<T> {
        match r {
            Ok(v) => v,
            Err(RelayIoError::Unreachable(_)) => {
                self.relays_unreachable.store(true, Ordering::Relaxed);
                Vec::new()
            }
        }
    }

    fn record_publish(&self, outcomes: &[(RelayUrl, PublishOutcome)]) {
        let failed = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Failed(_)))
            .count() as u32;
        if failed > 0 {
            self.publish_failed.fetch_add(failed, Ordering::Relaxed);
        }
    }
}

#[async_trait]
impl<R: RelayTransport, E: Entropy> TransportFacade for CharterTransportAdapter<R, E> {
    async fn publish_request(&self, request_json: &str, now: u64) {
        let outcomes = self.inner.submit_request(request_json, now).await;
        self.record_publish(&outcomes);
    }

    async fn poll_grants(&self, since: u64, now: u64) -> Vec<ReceivedGrant> {
        self.record_poll(self.inner.poll_grants(since, now).await)
    }

    async fn poll_clauses(&self, since: u64, now: u64) -> Vec<ReceivedClause> {
        self.record_poll(self.inner.poll_clauses(since, now).await)
    }

    async fn poll_usage_syncs(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)> {
        self.record_poll(self.inner.poll_usage_syncs(since, now).await)
    }

    async fn poll_releases(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)> {
        self.record_poll(self.inner.poll_releases(since, now).await)
    }

    async fn poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Vec<FetchedCuratorList> {
        self.record_poll(self.inner.poll_curator_lists(curators, since).await)
    }

    async fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) {
        let outcomes = self.inner.emit_audit(tags, now).await;
        self.record_publish(&outcomes);
    }

    fn pinned_guardian(&self) -> PubKey {
        self.inner.guardian_pubkey()
    }

    fn machine_pubkey(&self) -> PubKey {
        self.inner.machine_pubkey()
    }

    fn poll_health(&self) -> PollHealth {
        PollHealth {
            relays_unreachable: self.relays_unreachable.swap(false, Ordering::Relaxed),
            publish_failed: self.publish_failed.swap(0, Ordering::Relaxed),
        }
    }

    fn unwrap_local(&self, wrap: &NostrEvent, now: u64) -> Option<LocalInbound> {
        self.inner
            .unwrap_inbound(wrap, now)
            .map(|(ev, author)| classify_inbound(ev, author))
    }
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
    health: PollHealth,
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

    /// Test hook: make the next [`PollHealth`] read report every relay
    /// unreachable this round (simulating `RelayIoError::Unreachable`
    /// without needing a `Result`-returning trait method — see
    /// [`PollHealth`]'s doc comment for why).
    pub fn set_relays_unreachable(&self, v: bool) {
        self.inner.lock().expect("lock").health.relays_unreachable = v;
    }

    /// Test hook: make the next [`PollHealth`] read report `n` per-relay
    /// publish failures this round.
    pub fn set_publish_failed(&self, n: u32) {
        self.inner.lock().expect("lock").health.publish_failed = n;
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

    fn poll_health(&self) -> PollHealth {
        std::mem::take(&mut self.inner.lock().expect("lock").health)
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
