//! `CharterTransport` — the device's gift-wrapped pub/sub over a multi-relay
//! transport. Machine-side: wrap + publish REQUEST / AUDIT. Receive-side: poll
//! GRANT / CLAUSE wraps and return them **unverified** (charterd authenticates
//! via `charter-verify`). Delivery-only; never enacts.

use std::collections::BTreeMap;
use std::sync::Mutex;

use charter_content::CuratorList;
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
    /// rejected it. See B4, `internal/reviews/2026-09-21/01-core-crypto-proto.md`:
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

    /// Change the inbound cap (rate-limit / flood tests).
    pub fn with_max_inbound(mut self, cap: usize) -> Self {
        self.max_inbound = cap;
        self
    }

    /// The relay (for test inspection).
    pub fn relay(&self) -> &R {
        &self.relay
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
        match self.build_wrap(rumor, now) {
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
    pub fn build_request_wrap(&self, request_json: &str, now: u64) -> Option<NostrEvent> {
        self.build_wrap(
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
    pub fn build_status_wrap(&self, status_json: &str, now: u64) -> Option<NostrEvent> {
        self.build_wrap(
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

    /// Publish an already-built wrap (spool retry path).
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

    /// Emit a gift-wrapped AUDIT. **Content is always empty** (privacy); only
    /// routing/classification tags ride along.
    pub async fn emit_audit(
        &self,
        tags: Vec<Vec<String>>,
        now: u64,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        let mut all_tags = vec![kinds::marker_tag()];
        all_tags.extend(tags);
        let rumor = Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_AUDIT,
            tags: all_tags,
            content: String::new(), // always empty
            sig: None,
        };
        self.publish_rumor(&rumor, now).await
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
            if let Ok((rumor, seal_author)) =
                nip59::unwrap_with_author(&wrap, &self.machine_sk, now, nip59::MAX_JITTER_SECS)
            {
                if rumor.kind == inner_kind {
                    if let Some(ev) = rumor.into_signed_event() {
                        out.push((ev, seal_author));
                    }
                }
            }
        }
        Ok(out)
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
