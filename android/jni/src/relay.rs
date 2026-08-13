//! The warden's relay half: a blocking facade over the shared
//! `CharterTransport` (NIP-59 gift-wrap over `wss://`), sized for the
//! `Mutex<Warden>` shape — every call blocks on a dedicated current-thread
//! tokio runtime, so Kotlin must drive it from a worker thread, never main
//! (port-spec §2.3's pinned-caller rule; the full mpsc "charter-core" thread
//! lands with the request/grant broker).
//!
//! Generic over the `RelayTransport` port: production is
//! `RealRelayTransport` (rustls + compiled-in webpki roots, feature
//! `real-relay`); host tests inject `MockRelayTransport`.

use charter_primitives::PubKey;
use charter_sys::relay::{PublishOutcome, RelayTransport, RelayUrl};
use charter_transport::transport::{CharterTransport, Entropy, ReceivedClause};

/// Entropy over the OS RNG (bionic `getrandom(2)` on Android). Panics rather
/// than proceed with a predictable nonce — NIP-59 wrap keys must never be
/// guessable (the runtime.rs:85-98 rule).
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, buf: &mut [u8]) {
        getrandom::getrandom(buf).expect("OS entropy unavailable — refusing predictable nonces");
    }
}

/// The narrow, object-safe, blocking surface the warden drives. Object-safe
/// (and `Sync`, behind an `Arc`) so the poll orchestrator can run the network
/// half WITHOUT holding the global warden lock — a wedged relay must cost a
/// poll, never an enforcement tick (the hard-timeout lesson, hardening #14).
pub trait WardenRelay: Send + Sync {
    /// Pull delivered CLAUSE wraps since `since` (unverified — the warden's
    /// ingest path authenticates each one).
    fn poll_clauses(&self, since: u64, now: u64) -> Result<Vec<ReceivedClause>, String>;
    /// Pull delivered RELEASE events (guardian-signed unpair) since `since`,
    /// as `(event, seal_author)` — unverified; the warden runs verify_release.
    fn poll_releases(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<(charter_primitives::NostrEvent, PubKey)>, String>;
    /// Gift-wrap + publish a STATUS payload. Returns how many relays accepted;
    /// zero acceptances is an Err carrying the relays' rejection text.
    fn emit_status(&self, status_json: &str, now: u64) -> Result<usize, String>;
    /// Retry parked offline wraps (the §2.2 spool); returns (sent, remaining).
    fn drain_outbox(&self, now: u64) -> (usize, usize);
    /// Gift-wrap + publish a device AUDIT (kind 31000) to the guardian —
    /// tags only, content always empty. Used by the break-glass override so
    /// an emergency unlock is LOUD (contract §Break-glass override event).
    fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) -> Result<usize, String>;
    /// Bring-up diagnostic: how many RAW gift-wraps the relay holds for this
    /// machine since `since` — distinguishes "nothing arrives" from "wraps
    /// arrive but fail to unwrap" without exposing their contents.
    fn probe_raw_wraps(&self, since: u64) -> Result<usize, String>;
    /// The pinned relay set (for display/diagnostics).
    fn relays(&self) -> &[RelayUrl];
}

/// Blocking adapter over any `RelayTransport`: owns the transport and a
/// current-thread tokio runtime and `block_on`s each operation.
pub struct BlockingRelay<R: RelayTransport + Send + Sync + Clone> {
    transport: CharterTransport<R, OsEntropy>,
    /// A second handle to the same transport for raw-wrap probing (the
    /// orchestration layer hides raw counts by design).
    probe: R,
    machine_pk: PubKey,
    relays: Vec<RelayUrl>,
    rt: tokio::runtime::Runtime,
    /// The durable offline spool (§2.2), when a base dir was provided.
    outbox: Option<crate::outbox::Outbox>,
}

impl<R: RelayTransport + Send + Sync + Clone> BlockingRelay<R> {
    pub fn new(
        relay: R,
        machine_sk: [u8; 32],
        guardian: PubKey,
        relays: Vec<RelayUrl>,
    ) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;
        let machine_pk = PubKey::from_bytes(
            charter_crypto::xonly_pubkey(&machine_sk).map_err(|e| format!("machine sk: {e:?}"))?,
        );
        Ok(BlockingRelay {
            probe: relay.clone(),
            machine_pk,
            transport: CharterTransport::new(
                relay,
                OsEntropy,
                machine_sk,
                guardian,
                relays.clone(),
            ),
            relays,
            rt,
            outbox: None,
        })
    }

    /// Attach the durable offline spool rooted under `base`.
    pub fn with_outbox(mut self, base: &std::path::Path) -> Self {
        self.outbox = Some(crate::outbox::Outbox::new(base));
        self
    }
}

impl<R: RelayTransport + Send + Sync + Clone> WardenRelay for BlockingRelay<R> {
    fn poll_clauses(&self, since: u64, now: u64) -> Result<Vec<ReceivedClause>, String> {
        self.rt
            .block_on(self.transport.poll_clauses(since, now))
            .map_err(|e| format!("relay poll: {e:?}"))
    }

    fn poll_releases(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<(charter_primitives::NostrEvent, PubKey)>, String> {
        self.rt
            .block_on(self.transport.poll_releases(since, now))
            .map_err(|e| format!("relay poll (release): {e:?}"))
    }

    fn emit_status(&self, status_json: &str, now: u64) -> Result<usize, String> {
        // Build once so a total failure can park the EXACT wrap for retry.
        let Some(wrap) = self.transport.build_status_wrap(status_json, now) else {
            return Err("status wrap build failed".into());
        };
        let outcomes = self
            .rt
            .block_on(self.transport.publish_prebuilt(wrap.clone()));
        let accepted = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Ok))
            .count();
        if accepted == 0 {
            if let Some(outbox) = &self.outbox {
                let subject = serde_json::from_str::<serde_json::Value>(status_json)
                    .ok()
                    .and_then(|v| v.get("subject").and_then(|s| s.as_str().map(String::from)))
                    .unwrap_or_else(|| "unknown".into());
                outbox.put_status(&subject, &wrap, now);
            }
            let reasons: Vec<String> = outcomes
                .iter()
                .map(|(url, o)| match o {
                    PublishOutcome::Ok => format!("{url}: ok"),
                    PublishOutcome::Failed(e) => format!("{url}: {e}"),
                })
                .collect();
            return Err(format!(
                "status rejected everywhere: {}",
                reasons.join("; ")
            ));
        }
        Ok(accepted)
    }

    fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) -> Result<usize, String> {
        let outcomes = self.rt.block_on(self.transport.emit_audit(tags, now));
        let accepted = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Ok))
            .count();
        if accepted == 0 {
            return Err("audit rejected everywhere".into());
        }
        Ok(accepted)
    }

    fn drain_outbox(&self, now: u64) -> (usize, usize) {
        let Some(outbox) = &self.outbox else {
            return (0, 0);
        };
        let mut sent = 0;
        for (path, wrap) in outbox.entries(now) {
            let outcomes = self.rt.block_on(self.transport.publish_prebuilt(wrap));
            if outcomes
                .iter()
                .any(|(_, o)| matches!(o, PublishOutcome::Ok))
            {
                outbox.remove(&path);
                sent += 1;
            }
        }
        (sent, outbox.len())
    }

    fn probe_raw_wraps(&self, since: u64) -> Result<usize, String> {
        use charter_sys::relay::Filter;
        let filter = Filter {
            kinds: vec![charter_primitives::kinds::GIFT_WRAP],
            p_tags: vec![self.machine_pk],
            since: Some(since),
            ..Default::default()
        };
        self.rt
            .block_on(self.probe.query(&self.relays, filter))
            .map(|v| v.len())
            .map_err(|e| format!("probe: {e:?}"))
    }

    fn relays(&self) -> &[RelayUrl] {
        &self.relays
    }
}

/// Build the production relay (feature `real-relay`): the real `wss://` client
/// with rustls + compiled-in webpki roots — no OS cert store, no Google.
#[cfg(feature = "real-relay")]
pub fn real_relay(
    machine_sk: [u8; 32],
    guardian: PubKey,
    relays: Vec<RelayUrl>,
    base: &std::path::Path,
) -> Result<std::sync::Arc<dyn WardenRelay>, String> {
    let transport = charter_sys::relay::RealRelayTransport::default();
    Ok(std::sync::Arc::new(
        BlockingRelay::new(transport, machine_sk, guardian, relays)?.with_outbox(base),
    ))
}

// ---------------------------------------------------------------------------
// The broker's delivery seam.
// ---------------------------------------------------------------------------

/// Object-safe forwarding relay so ONE concrete facade type serves both the
/// production wss client and the host tests' in-memory mock (the `Warden`
/// stays non-generic).
pub struct DynRelay(pub Box<dyn RelayTransport + Send + Sync>);

#[async_trait::async_trait]
impl RelayTransport for DynRelay {
    async fn publish(
        &self,
        relays: &[RelayUrl],
        ev: charter_primitives::NostrEvent,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        self.0.publish(relays, ev).await
    }
    async fn query(
        &self,
        relays: &[RelayUrl],
        filter: charter_sys::relay::Filter,
    ) -> Result<Vec<charter_primitives::NostrEvent>, charter_sys::relay::RelayIoError> {
        self.0.query(relays, filter).await
    }
}

/// The spine broker's `TransportFacade` over the shared `CharterTransport`:
/// publish REQUESTs/AUDITs, poll GRANTs/CLAUSEs/curator lists — the exact
/// facade shape charterd's `RealTransportFacade` has on Linux.
pub struct AndroidTransportFacade {
    transport: CharterTransport<DynRelay, OsEntropy>,
    guardian: PubKey,
    machine: PubKey,
    /// The durable offline spool (§2.2) — REQUESTs parked on total failure.
    outbox: Option<crate::outbox::Outbox>,
}

impl AndroidTransportFacade {
    pub fn new(
        relay: Box<dyn RelayTransport + Send + Sync>,
        machine_sk: [u8; 32],
        guardian: PubKey,
        relays: Vec<RelayUrl>,
    ) -> Result<Self, String> {
        let machine = PubKey::from_bytes(
            charter_crypto::xonly_pubkey(&machine_sk).map_err(|e| format!("machine sk: {e:?}"))?,
        );
        Ok(AndroidTransportFacade {
            transport: CharterTransport::new(
                DynRelay(relay),
                OsEntropy,
                machine_sk,
                guardian,
                relays,
            ),
            guardian,
            machine,
            outbox: None,
        })
    }

    /// Attach the durable offline spool rooted under `base`.
    pub fn with_outbox(mut self, base: &std::path::Path) -> Self {
        self.outbox = Some(crate::outbox::Outbox::new(base));
        self
    }
}

#[async_trait::async_trait]
impl charter_spine::TransportFacade for AndroidTransportFacade {
    async fn publish_request(&self, request_json: &str, now: u64) {
        // Build once; if EVERY relay refuses, park the exact wrap for retry —
        // a child's ask made offline must reach the guardian when the network
        // returns, not silently vanish (§2.2). M16 already persisted the
        // Pending record before this call, so the record + spool re-converge.
        let Some(wrap) = self.transport.build_request_wrap(request_json, now) else {
            return;
        };
        let outcomes = self.transport.publish_prebuilt(wrap.clone()).await;
        let accepted = outcomes
            .iter()
            .any(|(_, o)| matches!(o, charter_sys::relay::PublishOutcome::Ok));
        if !accepted {
            if let Some(outbox) = &self.outbox {
                outbox.put_request(&wrap, now);
            }
        }
    }
    async fn poll_grants(&self, since: u64, now: u64) -> Vec<charter_transport::ReceivedGrant> {
        self.transport
            .poll_grants(since, now)
            .await
            .unwrap_or_default()
    }
    async fn poll_clauses(&self, since: u64, now: u64) -> Vec<charter_transport::ReceivedClause> {
        self.transport
            .poll_clauses(since, now)
            .await
            .unwrap_or_default()
    }
    async fn poll_usage_syncs(
        &self,
        since: u64,
        now: u64,
    ) -> Vec<(charter_primitives::NostrEvent, PubKey)> {
        self.transport
            .poll_usage_syncs(since, now)
            .await
            .unwrap_or_default()
    }
    /// RELEASE polling exists on this facade for the trait's sake, but the
    /// Android warden does NOT route its unpair through the broker: it polls
    /// releases itself in `Warden::poll_once` and applies them via
    /// `try_apply_releases`, which also owns the on-device teardown (clauses,
    /// pairing, restrictions, the next tick's inert decision).
    ///
    /// Returning the events here as well would hand the same release to two
    /// independent appliers. The spine path is the LINUX one (S7); this side
    /// deliberately stays quiet so there is exactly one place that unpairs an
    /// Android device.
    async fn poll_releases(
        &self,
        _since: u64,
        _now: u64,
    ) -> Vec<(charter_primitives::NostrEvent, PubKey)> {
        Vec::new()
    }
    async fn poll_curator_lists(
        &self,
        curators: &[PubKey],
        since: u64,
    ) -> Vec<charter_transport::FetchedCuratorList> {
        self.transport
            .poll_curator_lists(curators, since)
            .await
            .unwrap_or_default()
    }
    async fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) {
        let _ = self.transport.emit_audit(tags, now).await;
    }
    fn pinned_guardian(&self) -> PubKey {
        self.guardian
    }
    fn machine_pubkey(&self) -> PubKey {
        self.machine
    }
}
