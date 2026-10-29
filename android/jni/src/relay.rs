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

use std::sync::Arc;

use charter_link::LocalSink;
use charter_primitives::{PubKey, SignerSet};
use charter_spine::GuardianKeys;
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
    /// Gift-wrap + publish a STATUS payload, one wrap per recipient in the
    /// signer set (rule 9), on the pinned relays and `extra_relays` (rule 11:
    /// a ward with more than one root keeps sending STATUS on its previous
    /// relays for 7 days after a change).
    /// Returns how many relay acceptances there were; a wrap no relay took
    /// is parked and makes this an Err carrying the relays' rejection text.
    fn emit_status(
        &self,
        status_json: &str,
        now: u64,
        extra_relays: &[Vec<RelayUrl>],
    ) -> Result<usize, String>;
    /// Gift-wrap + publish a departed key's last STATUS (the body from
    /// `charter_spine::Farewell::to_json`) to `recipient` alone, on the
    /// pinned relays and `extra_relays`. Never parked: the guardian-keys
    /// state keeps it owed until a relay takes it. Returns whether one did.
    fn emit_farewell(
        &self,
        json: &str,
        now: u64,
        recipient: &PubKey,
        extra_relays: &[Vec<RelayUrl>],
    ) -> bool;
    /// Retry parked offline wraps (the §2.2 spool); returns (sent, remaining).
    /// A parked wrap addressed to a key no longer among the signer set's
    /// recipients is dropped, never re-addressed (review batch 2, platforms
    /// L4); with the guardian-keys state unreadable nothing is published.
    /// A dropped REQUEST wrap is not lost with it: while `is_open` says the
    /// request is still waiting, it is built afresh, one wrap per current
    /// recipient (a root that approved its first working key stops being a
    /// recipient while still pinned).
    fn drain_outbox(&self, now: u64, is_open: &dyn Fn(&str) -> bool) -> (usize, usize);
    /// Gift-wrap (without publishing) a device AUDIT (kind 31000), one wrap
    /// per recipient in the signer set (rule 9) — tags only, content always
    /// empty. Used by the break-glass override so an emergency unlock is LOUD
    /// (contract §Break-glass override event). NOT offered to the local
    /// link: the caller journals them first and then offers each with
    /// [`Self::offer_audit_local`], and retries each with
    /// [`Self::publish_wrap`].
    fn build_audit_wraps(
        &self,
        tags: Vec<Vec<String>>,
        now: u64,
    ) -> Vec<charter_primitives::NostrEvent>;
    /// Offer an AUDIT wrap from [`Self::build_audit_wrap`] to the local link,
    /// once it is journaled.
    fn offer_audit_local(&self, wrap: &charter_primitives::NostrEvent);
    /// Publish an already-built wrap verbatim (not offered to the local link
    /// again). Returns how many relays accepted; none is an Err.
    fn publish_wrap(&self, wrap: charter_primitives::NostrEvent) -> Result<usize, String>;
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
    /// The same identity with no local sink: builds the break-glass AUDIT
    /// wraps, which are offered to the link only once journaled
    /// ([`WardenRelay::offer_audit_local`]).
    audit_builder: CharterTransport<R, OsEntropy>,
    /// A second handle to the same transport for raw-wrap probing (the
    /// orchestration layer hides raw counts by design).
    probe: R,
    machine_pk: PubKey,
    relays: Vec<RelayUrl>,
    rt: tokio::runtime::Runtime,
    /// The durable offline spool (§2.2), when a base dir was provided.
    outbox: Option<crate::outbox::Outbox>,
    /// The ward's guardian keys: who a parked wrap may still go to.
    keys: Option<GuardianKeys>,
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
            audit_builder: CharterTransport::new(
                relay.clone(),
                OsEntropy,
                machine_sk,
                guardian,
                relays.clone(),
            ),
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
            keys: None,
        })
    }

    /// Attach the durable offline spool rooted under `base`.
    pub fn with_outbox(mut self, base: &std::path::Path) -> Self {
        self.outbox = Some(crate::outbox::Outbox::new(base));
        self
    }

    /// Address every wrap to the ward's guardian-keys recipients (rule 9):
    /// STATUS and AUDITs to the live signer set's recipients; a departed key
    /// owed its last STATUS gets [`WardenRelay::emit_farewell`] instead.
    /// Without keys (an unpaired build path) the pinned guardian alone, as
    /// before.
    pub fn with_guardians(mut self, keys: Option<&GuardianKeys>) -> Self {
        if let Some(keys) = keys {
            self.transport
                .set_signer_set(charter_spine::GuardianStatements::signers(keys));
            self.audit_builder
                .set_signer_set(charter_spine::GuardianStatements::signers(keys));
            self.keys = Some(keys.clone());
        }
        self
    }

    /// Attach the local link's outbox: every STATUS and AUDIT wrap this
    /// facade builds is offered to it before relay IO starts (the identical
    /// wrap, so both copies share one event id).
    pub fn with_local_sink(mut self, sink: Option<Arc<dyn LocalSink>>) -> Self {
        if let Some(sink) = sink {
            self.transport.set_local_sink(sink);
        }
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

    fn emit_status(
        &self,
        status_json: &str,
        now: u64,
        extra_relays: &[Vec<RelayUrl>],
    ) -> Result<usize, String> {
        // Build once so a total failure can park the EXACT wraps for retry:
        // one per recipient (each already offered to the local link).
        let wraps = self.transport.build_status_wraps(status_json, now);
        if wraps.is_empty() {
            return Err("status wrap build failed (no recipients)".into());
        }
        let subject = serde_json::from_str::<serde_json::Value>(status_json)
            .ok()
            .and_then(|v| v.get("subject").and_then(|s| s.as_str().map(String::from)))
            .unwrap_or_else(|| "unknown".into());
        let many = wraps.len() > 1;
        let mut accepted = 0;
        let mut reasons: Vec<String> = Vec::new();
        for wrap in wraps {
            let outcomes = self.rt.block_on(
                self.transport
                    .publish_prebuilt_on(std::slice::from_ref(&wrap), extra_relays),
            );
            let took = outcomes
                .iter()
                .filter(|(_, o)| matches!(o, PublishOutcome::Ok))
                .count();
            accepted += took;
            if took == 0 {
                if let Some(outbox) = &self.outbox {
                    // Latest-only per (subject, recipient): one recipient's
                    // parked STATUS never replaces another's.
                    let recipient = many.then(|| wrap_recipient(&wrap)).flatten();
                    outbox.put_status_for(&subject, recipient.as_deref(), &wrap, now);
                }
                reasons.extend(outcomes.iter().map(|(url, o)| match o {
                    PublishOutcome::Ok => format!("{url}: ok"),
                    PublishOutcome::Failed(e) => format!("{url}: {e}"),
                }));
            }
        }
        if !reasons.is_empty() {
            return Err(format!(
                "status rejected everywhere: {}",
                reasons.join("; ")
            ));
        }
        Ok(accepted)
    }

    fn build_audit_wraps(
        &self,
        tags: Vec<Vec<String>>,
        now: u64,
    ) -> Vec<charter_primitives::NostrEvent> {
        // `audit_builder` has no local sink: nothing is offered here.
        self.audit_builder.build_audit_wraps(tags, now)
    }

    fn offer_audit_local(&self, wrap: &charter_primitives::NostrEvent) {
        self.transport
            .offer_local(charter_primitives::kinds::CHARTER_DEVICE_AUDIT, None, wrap);
    }

    fn publish_wrap(&self, wrap: charter_primitives::NostrEvent) -> Result<usize, String> {
        let outcomes = self.rt.block_on(self.transport.publish_prebuilt(wrap));
        let accepted = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Ok))
            .count();
        if accepted == 0 {
            return Err("audit rejected everywhere".into());
        }
        Ok(accepted)
    }

    fn emit_farewell(
        &self,
        json: &str,
        now: u64,
        recipient: &PubKey,
        extra_relays: &[Vec<RelayUrl>],
    ) -> bool {
        self.rt
            .block_on(self.transport.emit_status_to(
                json,
                now,
                std::slice::from_ref(recipient),
                extra_relays,
            ))
            .iter()
            .any(|(_, o)| matches!(o, PublishOutcome::Ok))
    }

    fn drain_outbox(&self, now: u64, is_open: &dyn Fn(&str) -> bool) -> (usize, usize) {
        let Some(outbox) = &self.outbox else {
            return (0, 0);
        };
        let live: Option<Vec<String>> = match &self.keys {
            Some(k) if !k.is_available() => return (0, outbox.len()),
            Some(k) => Some(
                charter_spine::GuardianStatements::signers(k)
                    .recipients()
                    .iter()
                    .map(PubKey::to_hex)
                    .collect(),
            ),
            None => None,
        };
        let entries = outbox.entries(now);
        // Requests whose wrap to a departed key was dropped, to be built
        // afresh for the current recipients below.
        let mut rebuild: Vec<(crate::outbox::ParkedRequest, u64)> = Vec::new();
        let mut sent = 0;
        for parked in &entries {
            if let Some(live) = &live {
                if !wrap_recipient(&parked.wrap).is_some_and(|p| live.contains(&p)) {
                    outbox.remove(&parked.path);
                    if let Some(req) = &parked.request {
                        if !rebuild.iter().any(|(r, _)| r.req_id == req.req_id) {
                            rebuild.push((req.clone(), parked.at));
                        }
                    }
                    continue;
                }
            }
            let outcomes = self
                .rt
                .block_on(self.transport.publish_prebuilt(parked.wrap.clone()));
            if outcomes
                .iter()
                .any(|(_, o)| matches!(o, PublishOutcome::Ok))
            {
                outbox.remove(&parked.path);
                sent += 1;
            }
        }
        for (req, at) in rebuild {
            // Settled or expired: nothing more to ask.
            if !is_open(&req.req_id) {
                continue;
            }
            // A current recipient that had this ask parked (and has it still,
            // or was just sent it) gets no second wrap.
            let held = |p: &str| {
                entries.iter().any(|e| {
                    e.request.as_ref().is_some_and(|r| r.req_id == req.req_id)
                        && wrap_recipient(&e.wrap).as_deref() == Some(p)
                })
            };
            for wrap in self.transport.build_request_wraps(&req.json, now) {
                if wrap_recipient(&wrap).is_some_and(|p| held(&p)) {
                    continue;
                }
                let outcomes = self
                    .rt
                    .block_on(self.transport.publish_prebuilt(wrap.clone()));
                if outcomes
                    .iter()
                    .any(|(_, o)| matches!(o, PublishOutcome::Ok))
                {
                    sent += 1;
                } else {
                    outbox.put_request(&wrap, &req.json, at);
                }
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

/// The recipient a gift wrap is addressed to (its single `p` tag), as hex.
fn wrap_recipient(wrap: &charter_primitives::NostrEvent) -> Option<String> {
    wrap.tags
        .iter()
        .find(|t| t.first().is_some_and(|n| n == "p"))
        .and_then(|t| t.get(1).cloned())
}

/// Build the production relay (feature `real-relay`): the real `wss://` client
/// with rustls + compiled-in webpki roots — no OS cert store, no Google.
#[cfg(feature = "real-relay")]
pub fn real_relay(
    machine_sk: [u8; 32],
    guardian: PubKey,
    relays: Vec<RelayUrl>,
    base: &std::path::Path,
    local_sink: Option<Arc<dyn LocalSink>>,
    keys: Option<&GuardianKeys>,
) -> Result<std::sync::Arc<dyn WardenRelay>, String> {
    let transport =
        charter_sys::relay::RealRelayTransport::default().with_auth_signer(std::sync::Arc::new(
            charter_sys::signer::RealMachineSigner::from_secret(machine_sk),
        ));
    Ok(std::sync::Arc::new(
        BlockingRelay::new(transport, machine_sk, guardian, relays)?
            .with_outbox(base)
            .with_guardians(keys)
            .with_local_sink(local_sink),
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
///
/// Poll failures still surface as empty batches (offline is "nothing
/// delivered", never a crash), but they are no longer thrown away: an
/// unreachable relay set is recorded for [`poll_health`] to report once per
/// broker round, as charterd's facade does. Without it the guardian could not
/// tell "the phone is on a dead relay" from "nothing new" (parity row 8).
///
/// [`poll_health`]: charter_spine::TransportFacade::poll_health
pub struct AndroidTransportFacade {
    transport: CharterTransport<DynRelay, OsEntropy>,
    guardian: PubKey,
    machine: PubKey,
    /// The durable offline spool (§2.2) — REQUESTs parked on total failure.
    outbox: Option<crate::outbox::Outbox>,
    /// At least one poll this round reached NO relay in the set.
    relay_unreachable: std::sync::atomic::AtomicBool,
    /// Per-relay publish failures this round.
    publish_failed: std::sync::atomic::AtomicU32,
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
            relay_unreachable: std::sync::atomic::AtomicBool::new(false),
            publish_failed: std::sync::atomic::AtomicU32::new(0),
        })
    }

    /// Attach the durable offline spool rooted under `base`.
    pub fn with_outbox(mut self, base: &std::path::Path) -> Self {
        self.outbox = Some(crate::outbox::Outbox::new(base));
        self
    }

    /// Attach the local link's outbox: every REQUEST and AUDIT wrap the
    /// broker publishes is offered to it before relay IO starts.
    pub fn with_local_sink(mut self, sink: Option<Arc<dyn LocalSink>>) -> Self {
        if let Some(sink) = sink {
            self.transport.set_local_sink(sink);
        }
        self
    }

    /// Address every REQUEST and AUDIT wrap to the signer set's recipients
    /// (rule 9), one wrap each. The set is live, so an approval or a
    /// departure is followed from the next wrap on.
    pub fn with_signer_set(mut self, signers: Arc<dyn SignerSet>) -> Self {
        self.transport.set_signer_set(signers);
        self
    }

    /// A poll's batch, with an unreachable relay set noted for
    /// [`charter_spine::TransportFacade::poll_health`] rather than swallowed.
    fn record_poll<T>(&self, r: Result<Vec<T>, charter_sys::relay::RelayIoError>) -> Vec<T> {
        match r {
            Ok(v) => v,
            Err(charter_sys::relay::RelayIoError::Unreachable(_)) => {
                self.relay_unreachable
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Vec::new()
            }
        }
    }

    /// Fold a publish's per-relay `Failed` outcomes into the round's count.
    fn record_publish(&self, outcomes: &[(RelayUrl, PublishOutcome)]) {
        let failed = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Failed(_)))
            .count() as u32;
        if failed > 0 {
            self.publish_failed
                .fetch_add(failed, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[async_trait::async_trait]
impl charter_spine::TransportFacade for AndroidTransportFacade {
    async fn publish_request(&self, request_json: &str, now: u64) {
        // Build once, one wrap per recipient (rule 9); if EVERY relay
        // refuses one, park that exact wrap for retry — a child's ask made
        // offline must reach the guardian when the network returns, not
        // silently vanish (§2.2). M16 already persisted the Pending record
        // before this call, so the record + spool re-converge.
        for wrap in self.transport.build_request_wraps(request_json, now) {
            let outcomes = self.transport.publish_prebuilt(wrap.clone()).await;
            self.record_publish(&outcomes);
            let accepted = outcomes
                .iter()
                .any(|(_, o)| matches!(o, charter_sys::relay::PublishOutcome::Ok));
            if !accepted {
                if let Some(outbox) = &self.outbox {
                    outbox.put_request(&wrap, request_json, now);
                }
            }
        }
    }
    async fn poll_grants(&self, since: u64, now: u64) -> Vec<charter_transport::ReceivedGrant> {
        self.record_poll(self.transport.poll_grants(since, now).await)
    }
    async fn poll_clauses(&self, since: u64, now: u64) -> Vec<charter_transport::ReceivedClause> {
        self.record_poll(self.transport.poll_clauses(since, now).await)
    }
    async fn poll_usage_syncs(
        &self,
        since: u64,
        now: u64,
    ) -> Vec<(charter_primitives::NostrEvent, PubKey)> {
        self.record_poll(self.transport.poll_usage_syncs(since, now).await)
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
        self.record_poll(self.transport.poll_curator_lists(curators, since).await)
    }
    async fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) {
        let outcomes = self.transport.emit_audit(tags, now).await;
        self.record_publish(&outcomes);
    }
    /// Guardian statements (31120) and presence confirmations (31121),
    /// opened with the seal-exempt unwrap, for the broker's guardian-keys
    /// port.
    async fn poll_guardian_events(
        &self,
        since: u64,
        now: u64,
    ) -> Vec<charter_transport::GuardianSealed> {
        self.record_poll(self.transport.poll_guardian_events(since, now).await)
    }
    /// Gift-wrapped RELAYS (rule 11), with any guardian PIN they carry: the
    /// broker records the PIN in the guardian-keys state, never the pairing.
    async fn poll_relays(
        &self,
        since: u64,
        now: u64,
    ) -> Vec<(charter_primitives::NostrEvent, PubKey)> {
        self.record_poll(self.transport.poll_relays(since, now).await)
    }
    fn pinned_guardian(&self) -> PubKey {
        self.guardian
    }
    fn machine_pubkey(&self) -> PubKey {
        self.machine
    }
    /// A wrap from the local link, opened by the same NIP-59 code as a polled
    /// one and sorted by kind. A RELEASE is handed back here like any other
    /// kind; the warden asks the broker NOT to apply it
    /// (`handle_release: false`) and runs its own `try_apply_releases`, so
    /// there is still exactly one place that unpairs an Android device.
    fn unwrap_local(
        &self,
        wrap: &charter_primitives::NostrEvent,
        now: u64,
    ) -> Option<charter_spine::LocalInbound> {
        // The opened unwrap also sorts out gift-wrapped RELAYS and guardian
        // statements / presence confirmations (seal-exempt, both authors).
        self.transport
            .unwrap_inbound_opened(wrap, now)
            .map(charter_spine::transport_facade::classify_opened)
    }
    /// Read-and-reset, once per broker `poll_once` round.
    fn poll_health(&self) -> charter_spine::transport_facade::PollHealth {
        charter_spine::transport_facade::PollHealth {
            relays_unreachable: self
                .relay_unreachable
                .swap(false, std::sync::atomic::Ordering::Relaxed),
            publish_failed: self
                .publish_failed
                .swap(0, std::sync::atomic::Ordering::Relaxed),
        }
    }
}
