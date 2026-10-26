//! `CharterTransport` — the device's gift-wrapped pub/sub over a multi-relay
//! transport. Machine-side: wrap + publish REQUEST / AUDIT. Receive-side: poll
//! GRANT / CLAUSE wraps and return them **unverified** (charterd authenticates
//! via `charter-verify`). Delivery-only; never enacts.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use charter_content::CuratorList;
use charter_link::{LocalOffer, LocalSink};
use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_sys::relay::{Filter, PublishOutcome, RelayIoError, RelayTransport, RelayUrl};
use zeroize::Zeroizing;

use crate::curator::{list_id, parse_curator_list};
use crate::nip59::{self, Rumor, WrapRandomness};

/// Injected randomness seam (ephemeral keys + nonces). Production uses an OS
/// RNG; tests use a deterministic `ScriptedEntropy`.
pub trait Entropy: Send + Sync {
    fn fill(&self, buf: &mut [u8]);
}

/// Deterministic entropy for tests (a simple counter stream).
pub struct ScriptedEntropy {
    ctr: Mutex<u64>,
}

impl ScriptedEntropy {
    pub fn new(seed: u64) -> Self {
        ScriptedEntropy {
            ctr: Mutex::new(seed),
        }
    }
}

impl Default for ScriptedEntropy {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Entropy for ScriptedEntropy {
    fn fill(&self, buf: &mut [u8]) {
        let mut c = self.ctr.lock().expect("entropy lock");
        for b in buf.iter_mut() {
            *c = c.wrapping_add(0x9E37_79B9_7F4A_7C15);
            *b = (*c >> 24) as u8;
        }
        // Guarantee a non-zero scalar.
        if buf.iter().all(|&x| x == 0) {
            if let Some(first) = buf.first_mut() {
                *first = 1;
            }
        }
    }
}

/// Why [`CharterTransport::try_new`] could not construct a transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    /// The supplied machine secret is not a valid secp256k1 scalar (all-zero,
    /// out of range, or otherwise corrupted) — `charter_crypto::xonly_pubkey`
    /// rejected it. See B4, the 2026-09-21 review:
    /// a partial write, a zero-filled restore, or filesystem damage on a device
    /// that loses power routinely can all leave a stored secret in this state,
    /// and the caller must be able to keep enforcing cached clauses rather than
    /// abort.
    BadMachineSecret,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::BadMachineSecret => write!(f, "machine secret is not a valid key"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A received (but not yet verified) GRANT.
#[derive(Debug, Clone)]
pub struct ReceivedGrant {
    pub grant: NostrEvent,
    pub seal_author: PubKey,
}

/// A received (but not yet verified) CLAUSE.
#[derive(Debug, Clone)]
pub struct ReceivedClause {
    pub clause: NostrEvent,
    pub seal_author: PubKey,
}

/// A fetched, signature-verified curator web list. Replaceable: keyed by
/// `(curator, list_id)`, newest `created_at` wins.
#[derive(Debug, Clone)]
pub struct FetchedCuratorList {
    pub curator: PubKey,
    pub list_id: String,
    pub created_at: u64,
    pub list: CuratorList,
}

/// A PAIR_OFFER delivered to this machine, with the authenticated key that
/// sealed it. `seal_author` is the ONLY key safe to pin — the payload never
/// names a guardian.
#[derive(Debug, Clone)]
pub struct ReceivedPairOffer {
    pub offer: crate::pair_offer::PairOffer,
    pub seal_author: PubKey,
}

/// Default [`CharterTransport`] inbound cap: eight relays' worth of the
/// per-relay fetch ceiling (`charter-sys` `MAX_EVENTS_PER_QUERY`, 1024), so a
/// poll never discards a fetched event without looking at it.
pub const DEFAULT_MAX_INBOUND: usize = 8 * 1024;

/// The device transport. Holds the machine secret (for ECDH + sealing) and the
/// pinned guardian pubkey + relays.
pub struct CharterTransport<R: RelayTransport, E: Entropy> {
    relay: R,
    entropy: E,
    /// Wrapped in [`Zeroizing`]: this is the ECDH half that decrypts every
    /// guardian message, held for the transport's whole lifetime.
    machine_sk: Zeroizing<[u8; 32]>,
    machine_pk: PubKey,
    guardian_pk: PubKey,
    relays: Vec<RelayUrl>,
    /// Cap on inbound events processed per poll (DoS hardening). It must sit
    /// ABOVE what a poll can fetch (`charter-sys` buffers at most 1024 events
    /// per relay): the recipient key in a wrap's `p` tag is public, so anyone —
    /// the ward included — can publish junk wraps at it, and a cap below the
    /// fetch ceiling was applied in relay-delivery order BEFORE any unwrap, so
    /// a few hundred fresh junk wraps pushed every genuine clause, stand-down
    /// and release past the cut, poll after poll. An unwrap attempt is one
    /// schnorr verify plus one ECDH; the whole fetch ceiling is a fraction of a
    /// second, so there is nothing to gain by discarding events unexamined.
    max_inbound: usize,
    /// Where every machine→guardian wrap this transport builds is offered for
    /// direct delivery over the local link, before any relay IO starts. `None`
    /// (the default) keeps the historic relay-only behaviour.
    local_sink: Option<Arc<dyn LocalSink>>,
}

impl<R: RelayTransport, E: Entropy> CharterTransport<R, E> {
    /// Fallible constructor: `Err(TransportError::BadMachineSecret)` when the
    /// stored machine secret does not derive a valid key, instead of aborting
    /// the process. The caller (e.g. `charterd`) can then keep enforcing
    /// cached clauses and report the failure in STATUS rather than fail
    /// open by crashing (B4).
    pub fn try_new(
        relay: R,
        entropy: E,
        machine_sk: [u8; 32],
        guardian_pk: PubKey,
        relays: Vec<RelayUrl>,
    ) -> Result<Self, TransportError> {
        let machine_pk = PubKey::from_bytes(
            charter_crypto::xonly_pubkey(&machine_sk)
                .map_err(|_| TransportError::BadMachineSecret)?,
        );
        Ok(CharterTransport {
            relay,
            entropy,
            machine_sk: Zeroizing::new(machine_sk),
            machine_pk,
            guardian_pk,
            relays,
            max_inbound: DEFAULT_MAX_INBOUND,
            local_sink: None,
        })
    }

    /// Panicking wrapper over [`Self::try_new`], kept for callers (the
    /// android/jni warden, this crate's own tests) that have always treated
    /// an unusable machine secret as unrecoverable. New callers — anything
    /// that can instead keep enforcing cached clauses — should use `try_new`.
    pub fn new(
        relay: R,
        entropy: E,
        machine_sk: [u8; 32],
        guardian_pk: PubKey,
        relays: Vec<RelayUrl>,
    ) -> Self {
        Self::try_new(relay, entropy, machine_sk, guardian_pk, relays)
            .expect("valid machine secret")
    }

    /// This transport's own machine pubkey (derived from the machine secret
    /// at construction).
    pub fn machine_pubkey(&self) -> PubKey {
        self.machine_pk
    }

    /// The pinned guardian this transport's wraps are addressed to.
    pub fn guardian_pubkey(&self) -> PubKey {
        self.guardian_pk
    }

    /// Change the inbound cap (rate-limit / flood tests).
    pub fn with_max_inbound(mut self, cap: usize) -> Self {
        self.max_inbound = cap;
        self
    }

    /// The relay (for test inspection).
    pub fn relay(&self) -> &R {
        &self.relay
    }

    /// Attach the local-link outbox. From then on every machine→guardian wrap
    /// this transport builds (REQUEST, STATUS, AUDIT) is offered to `sink`
    /// once, at build time, before relay IO starts — the identical wrap the
    /// relays receive, so both copies share one event id. Offering never
    /// changes relay publish outcomes: the sink must not block on the network
    /// (`LocalSink::offer`'s contract), and nothing here waits on it.
    pub fn set_local_sink(&mut self, sink: Arc<dyn LocalSink>) {
        self.local_sink = Some(sink);
    }

    /// Builder form of [`Self::set_local_sink`], for construction sites that
    /// build the transport by value.
    pub fn with_local_sink(mut self, sink: Arc<dyn LocalSink>) -> Self {
        self.set_local_sink(sink);
        self
    }

    /// Offer an already-built machine→guardian wrap to the local sink, if one
    /// is attached. Wraps built by this transport are offered automatically;
    /// this is for a platform's own durable outbox, which re-publishes parked
    /// wraps through [`Self::publish_prebuilt`] (deliberately NOT an offer:
    /// it cannot tell what is inside the wrap, and the wrap was offered when it
    /// was built) and knows each one's inner kind.
    pub fn offer_local(&self, inner_kind: u16, key: Option<String>, wrap: &NostrEvent) {
        if let Some(sink) = &self.local_sink {
            sink.offer(LocalOffer {
                inner_kind,
                key,
                wrap: wrap.clone(),
            });
        }
    }

    /// The local-outbox replacement key for a rumor: STATUS is "latest per
    /// subject" (`status:<subject hex>`), everything else is kept as is.
    fn local_key(rumor: &Rumor) -> Option<String> {
        if rumor.kind != kinds::CHARTER_DEVICE_STATUS {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&rumor.content)
            .ok()?
            .get("subject")?
            .as_str()
            .map(|subject| format!("status:{subject}"))
    }

    /// Build the wrap for a machine→guardian rumor and offer it to the local
    /// sink. Every outbound path that builds a wrap to the guardian goes
    /// through here, so none can skip the local offer.
    fn build_and_offer(&self, rumor: &Rumor, now: u64) -> Option<NostrEvent> {
        let wrap = self.build_wrap(rumor, now)?;
        self.offer_local(rumor.kind, Self::local_key(rumor), &wrap);
        Some(wrap)
    }

    fn randomness(&self, now: u64) -> WrapRandomness {
        let mut ephemeral_secret = [0u8; 32];
        let mut seal_nonce = [0u8; 32];
        let mut wrap_nonce = [0u8; 32];
        self.entropy.fill(&mut ephemeral_secret);
        self.entropy.fill(&mut seal_nonce);
        self.entropy.fill(&mut wrap_nonce);
        WrapRandomness {
            ephemeral_secret,
            seal_nonce,
            wrap_nonce,
            seal_created_at: now,
            wrap_created_at: now,
        }
    }

    async fn publish_rumor(&self, rumor: &Rumor, now: u64) -> Vec<(RelayUrl, PublishOutcome)> {
        // A PAIR_OFFER is the only rumor this type sends that is not
        // machine→guardian (a guardian-side transport addressing an unpaired
        // ward, which has no local link yet): it is never offered.
        let wrap = if rumor.kind == kinds::CHARTER_DEVICE_PAIR_OFFER {
            self.build_wrap(rumor, now)
        } else {
            self.build_and_offer(rumor, now)
        };
        match wrap {
            Some(wrap) => self.relay.publish(&self.relays, wrap).await,
            None => vec![(
                "<local>".to_string(),
                PublishOutcome::Failed("wrap failed".into()),
            )],
        }
    }

    /// Build the gift-wrap for a rumor WITHOUT publishing — the seam an
    /// offline spool needs: a wrap built once can be retried later verbatim
    /// (mobile offline windows are routine, port-spec §2.2).
    fn build_wrap(&self, rumor: &Rumor, now: u64) -> Option<NostrEvent> {
        let r = self.randomness(now);
        nip59::wrap(rumor, &self.machine_sk, self.guardian_pk.as_bytes(), &r).ok()
    }

    /// Build (without publishing) the gift-wrapped REQUEST for later retry.
    /// The wrap is offered to the local sink (if any) here, at build time:
    /// callers build only what they are about to publish.
    pub fn build_request_wrap(&self, request_json: &str, now: u64) -> Option<NostrEvent> {
        self.build_and_offer(
            &Rumor {
                id: None,
                pubkey: self.machine_pk,
                created_at: now,
                kind: kinds::CHARTER_DEVICE_REQUEST,
                tags: vec![kinds::marker_tag()],
                content: request_json.to_string(),
                sig: None,
            },
            now,
        )
    }

    /// Build (without publishing) the gift-wrapped STATUS for later retry.
    /// Offered to the local sink at build time, like
    /// [`Self::build_request_wrap`].
    pub fn build_status_wrap(&self, status_json: &str, now: u64) -> Option<NostrEvent> {
        self.build_and_offer(
            &Rumor {
                id: None,
                pubkey: self.machine_pk,
                created_at: now,
                kind: kinds::CHARTER_DEVICE_STATUS,
                tags: vec![kinds::marker_tag()],
                content: status_json.to_string(),
                sig: None,
            },
            now,
        )
    }

    /// Publish an already-built wrap (spool retry path). Not offered to the
    /// local sink: a wrap built here was offered when it was built, and a
    /// platform outbox re-offers its parked wraps with [`Self::offer_local`].
    pub async fn publish_prebuilt(&self, wrap: NostrEvent) -> Vec<(RelayUrl, PublishOutcome)> {
        self.relay.publish(&self.relays, wrap).await
    }

    /// Wrap + publish a REQUEST (content = the request payload JSON).
    pub async fn submit_request(
        &self,
        request_json: &str,
        now: u64,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        let rumor = Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_REQUEST,
            tags: vec![kinds::marker_tag()],
            content: request_json.to_string(),
            sig: None,
        };
        self.publish_rumor(&rumor, now).await
    }

    /// The AUDIT rumor: the marker plus `tags`. **Content is always empty**
    /// (privacy); only routing/classification tags ride along.
    fn audit_rumor(&self, tags: Vec<Vec<String>>, now: u64) -> Rumor {
        let mut all_tags = vec![kinds::marker_tag()];
        all_tags.extend(tags);
        Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_AUDIT,
            tags: all_tags,
            content: String::new(), // always empty
            sig: None,
        }
    }

    /// Emit a gift-wrapped AUDIT. **Content is always empty** (privacy); only
    /// routing/classification tags ride along.
    pub async fn emit_audit(
        &self,
        tags: Vec<Vec<String>>,
        now: u64,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        let rumor = self.audit_rumor(tags, now);
        self.publish_rumor(&rumor, now).await
    }

    /// Build (without publishing) the gift-wrapped AUDIT, for a platform
    /// that journals an audit until a relay takes it: build it ONCE, keep the
    /// wrap with the journal entry and retry it with
    /// [`Self::publish_prebuilt`]. Offered to the local sink here, at build
    /// time, like [`Self::build_request_wrap`] — so the guardian sees one
    /// event id however many rounds the relays stay down.
    pub fn build_audit_wrap(&self, tags: Vec<Vec<String>>, now: u64) -> Option<NostrEvent> {
        self.build_and_offer(&self.audit_rumor(tags, now), now)
    }

    /// Wrap + publish a STATUS (kind 31114, content = the `StatusPayload` JSON),
    /// gift-wrapped to the guardian. One per child per device; the payload is
    /// numbers + enums only (no PII) — see `charter_proto::StatusPayload`.
    pub async fn emit_status(
        &self,
        status_json: &str,
        now: u64,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        let rumor = Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_STATUS,
            tags: vec![kinds::marker_tag()],
            content: status_json.to_string(),
            sig: None,
        };
        self.publish_rumor(&rumor, now).await
    }

    /// Guardian side: offer to be pinned by the ward this transport addresses,
    /// proving physical sight of its screen by echoing the `token` from its
    /// on-screen pairing QR.
    pub async fn send_pair_offer(&self, token: &str, now: u64) -> Vec<(RelayUrl, PublishOutcome)> {
        let content = crate::pair_offer::build_pair_offer(token, &self.relays, now);
        let rumor = Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_PAIR_OFFER,
            tags: vec![kinds::marker_tag()],
            content,
            sig: None,
        };
        self.publish_rumor(&rumor, now).await
    }

    /// Ward side: pairing offers addressed to this machine, each paired with
    /// the key that sealed it.
    ///
    /// A malformed payload is dropped silently rather than raised: an unpaired
    /// ward is an open mailbox on a public relay, and must not be knocked over
    /// by junk posted to it.
    pub async fn poll_pair_offers(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<ReceivedPairOffer>, RelayIoError> {
        let filter = Filter {
            kinds: vec![kinds::GIFT_WRAP],
            p_tags: vec![self.machine_pk],
            since: Some(since),
            ..Default::default()
        };
        let wraps = self.relay.query(&self.relays, filter).await?;
        let mut out = Vec::new();
        for wrap in wraps.into_iter().take(self.max_inbound) {
            if let Ok((rumor, seal_author)) =
                nip59::unwrap_with_author(&wrap, &self.machine_sk, now, nip59::MAX_JITTER_SECS)
            {
                if rumor.kind == kinds::CHARTER_DEVICE_PAIR_OFFER {
                    if let Some(offer) = crate::pair_offer::parse_pair_offer(&rumor.content) {
                        out.push(ReceivedPairOffer { offer, seal_author });
                    }
                }
            }
        }
        Ok(out)
    }

    async fn poll_inner(
        &self,
        inner_kind: u16,
        since: u64,
        now: u64,
    ) -> Result<Vec<(NostrEvent, PubKey)>, RelayIoError> {
        let filter = Filter {
            kinds: vec![kinds::GIFT_WRAP],
            p_tags: vec![self.machine_pk],
            since: Some(since),
            ..Default::default()
        };
        let wraps = self.relay.query(&self.relays, filter).await?;
        let mut out = Vec::new();
        for wrap in wraps.into_iter().take(self.max_inbound) {
            // A tampered/foreign wrap is non-fatal — skip it.
            if let Some((ev, seal_author)) = self.open_wrap(&wrap, now) {
                if ev.kind == inner_kind {
                    out.push((ev, seal_author));
                }
            }
        }
        Ok(out)
    }

    /// The one unwrap every inbound guardian wrap goes through, whichever way
    /// it arrived: NIP-59 unwrap under the machine key with the transport's
    /// jitter bound, then the inner event must carry its own id and signature
    /// (a bare rumor is dropped). Returns the inner event **unverified** with
    /// the key that sealed it; `charter-verify` authenticates it downstream.
    fn open_wrap(&self, wrap: &NostrEvent, now: u64) -> Option<(NostrEvent, PubKey)> {
        let (rumor, seal_author) =
            nip59::unwrap_with_author(wrap, &self.machine_sk, now, nip59::MAX_JITTER_SECS).ok()?;
        let ev = rumor.into_signed_event()?;
        Some((ev, seal_author))
    }

    /// Unwrap one gift wrap that arrived outside relay polling (the local
    /// link), through exactly the code [`Self::poll_grants`] and its siblings
    /// use. The wrap must name this machine in a single `p` tag — the relay
    /// path gets that from its `#p` filter, so it is checked here instead.
    /// Returns the inner event (any kind) **unverified**, with its seal
    /// author; `None` for anything that does not open.
    pub fn unwrap_inbound(&self, wrap: &NostrEvent, now: u64) -> Option<(NostrEvent, PubKey)> {
        let me = self.machine_pk.to_hex();
        let mut p_tags = wrap
            .tags
            .iter()
            .filter(|t| t.first().is_some_and(|n| n == "p"));
        match (p_tags.next(), p_tags.next()) {
            (Some(t), None) if t.get(1).is_some_and(|v| *v == me) => {}
            _ => return None,
        }
        self.open_wrap(wrap, now)
    }

    /// Poll for delivered GRANT wraps (unverified).
    pub async fn poll_grants(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<ReceivedGrant>, RelayIoError> {
        Ok(self
            .poll_inner(kinds::CHARTER_DEVICE_GRANT, since, now)
            .await?
            .into_iter()
            .map(|(grant, seal_author)| ReceivedGrant { grant, seal_author })
            .collect())
    }

    /// Poll for delivered RELEASE wraps (kind 31116) — the guardian-signed
    /// unpair. Returns `(event, seal_author)` unverified; the warden runs
    /// `verify_release`.
    pub async fn poll_releases(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<(NostrEvent, PubKey)>, RelayIoError> {
        self.poll_inner(kinds::CHARTER_DEVICE_RELEASE, since, now)
            .await
    }

    /// Poll for delivered USAGE_SYNC wraps (kind 31115) — the guardian-signed
    /// consolidated cross-device usage view. Returns `(event, seal_author)`
    /// unverified; the warden runs `verify_usage_sync`.
    pub async fn poll_usage_syncs(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<(NostrEvent, PubKey)>, RelayIoError> {
        self.poll_inner(kinds::CHARTER_DEVICE_USAGE_SYNC, since, now)
            .await
    }

    /// Poll for delivered CLAUSE wraps (unverified).
    pub async fn poll_clauses(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<ReceivedClause>, RelayIoError> {
        Ok(self
            .poll_inner(kinds::CHARTER_DEVICE_CLAUSE, since, now)
            .await?
            .into_iter()
            .map(|(clause, seal_author)| ReceivedClause {
                clause,
                seal_author,
            })
            .collect())
    }

    /// Fetch subscribed curators' signed web lists (kind 30100). Rejects any
    /// event whose author is not in `curators` (a hostile relay may ignore the
    /// filter), whose id is inconsistent, or whose signature is invalid; parses
    /// survivors; keeps the newest event per `(curator, list_id)`. A hostile
    /// relay can drop or reorder but never inject a list the device accepts.
    pub async fn poll_curator_lists(
        &self,
        curators: &[PubKey],
        since: u64,
    ) -> Result<Vec<FetchedCuratorList>, RelayIoError> {
        if curators.is_empty() {
            return Ok(Vec::new());
        }
        let filter = Filter {
            kinds: vec![kinds::CHARTER_CURATOR_WEB_LIST],
            authors: curators.to_vec(),
            since: Some(since),
            ..Default::default()
        };
        let events = self.relay.query(&self.relays, filter).await?;
        let mut newest: BTreeMap<(String, String), FetchedCuratorList> = BTreeMap::new();
        for ev in events.into_iter().take(self.max_inbound) {
            if !curators.contains(&ev.pubkey) {
                continue;
            }
            if !charter_verify::id_is_consistent(&ev) || !charter_verify::signature_is_valid(&ev) {
                continue;
            }
            let Some(list) = parse_curator_list(&ev) else {
                continue;
            };
            let key = (ev.pubkey.to_hex(), list_id(&ev));
            let keep = match newest.get(&key) {
                Some(existing) => ev.created_at > existing.created_at,
                None => true,
            };
            if keep {
                newest.insert(
                    key.clone(),
                    FetchedCuratorList {
                        curator: ev.pubkey,
                        list_id: key.1,
                        created_at: ev.created_at,
                        list,
                    },
                );
            }
        }
        Ok(newest.into_values().collect())
    }
}
