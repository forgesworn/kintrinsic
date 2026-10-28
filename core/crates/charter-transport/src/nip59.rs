//! NIP-59 gift-wrap (seal + wrap) over NIP-44 v2. The wrap (kind 1059) hides
//! the seal (kind 13) which hides the rumor. `unwrap` enforces the wrap/seal
//! kinds and signatures, the `rumor.pubkey == seal.pubkey` author binding, and
//! a jitter bound. A hostile relay can drop/delay a wrap but never forge the
//! sealed author.
//!
//! The core functions take explicit randomness (ephemeral key + nonces +
//! timestamps) so they are deterministic and testable; the transport layer
//! injects the entropy/clock seam.

use serde::{Deserialize, Serialize};

use charter_primitives::{kinds, EventId, NostrEvent, PubKey, Sig};
use charter_proto::canonical_event_string;

use crate::nip44;

/// Two days of jitter, in seconds — the NIP-59 backdating bound.
pub const MAX_JITTER_SECS: u64 = 2 * 24 * 60 * 60;

/// The innermost event. For machine->guardian messages (request/audit) the
/// `sig` is absent (the seal author vouches). For guardian->machine messages
/// (grant/clause) the inner guardian `sig` is **preserved** so `verify_grant` /
/// `verify_clause` can authenticate it independently (the SDK's `createRumor`
/// strips sig — Charter delivery keeps it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rumor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<EventId>,
    pub pubkey: PubKey,
    pub created_at: u64,
    pub kind: u16,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<Sig>,
}

impl Rumor {
    /// A signed event as a rumor (preserving id + sig) — the delivery form of a
    /// guardian-signed grant/clause.
    pub fn from_signed_event(ev: &NostrEvent) -> Rumor {
        Rumor {
            id: Some(ev.id),
            pubkey: ev.pubkey,
            created_at: ev.created_at,
            kind: ev.kind,
            tags: ev.tags.clone(),
            content: ev.content.clone(),
            sig: Some(ev.sig),
        }
    }

    /// Reconstruct the signed event (requires id + sig present).
    pub fn into_signed_event(self) -> Option<NostrEvent> {
        Some(NostrEvent {
            id: self.id?,
            pubkey: self.pubkey,
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags,
            content: self.content,
            sig: self.sig?,
        })
    }
}

/// NIP-59 errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nip59Error {
    WrongWrapKind,
    WrongSealKind,
    BadWrapSignature,
    BadSealSignature,
    /// `rumor.pubkey != seal.pubkey` — a forged author.
    AuthorMismatch,
    /// The wrap timestamp is implausibly far from now.
    JitterTooLarge,
    /// A NIP-44 decrypt failed (wrong recipient / tampered).
    Decrypt(nip44::Nip44Error),
    /// JSON shape error.
    Json,
    /// A key was invalid.
    BadKey,
    /// A seal-exempt inner event (see [`SEAL_EXEMPT_KINDS`]) was not a signed
    /// event, or its own id or signature did not verify.
    BadInnerEvent,
}

/// Sign an arbitrary event with a raw secret key (computes the NIP-01 id, then
/// schnorr-signs it). Used for the seal (author key) and wrap (ephemeral key).
pub fn sign_event_raw(
    secret32: &[u8; 32],
    kind: u16,
    created_at: u64,
    tags: Vec<Vec<String>>,
    content: String,
) -> Result<NostrEvent, Nip59Error> {
    let pk = charter_crypto::xonly_pubkey(secret32).map_err(|_| Nip59Error::BadKey)?;
    let mut ev = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey: PubKey::from_bytes(pk),
        created_at,
        kind,
        tags,
        content,
        sig: Sig::from_bytes([0; 64]),
    };
    let id = charter_crypto::sha256(canonical_event_string(&ev).as_bytes());
    ev.id = EventId::from_bytes(id);
    let sig = charter_crypto::schnorr_sign(secret32, &id).map_err(|_| Nip59Error::BadKey)?;
    ev.sig = Sig::from_bytes(sig);
    Ok(ev)
}

fn sig_valid(ev: &NostrEvent) -> bool {
    let id = charter_crypto::sha256(canonical_event_string(ev).as_bytes());
    if EventId::from_bytes(id) != ev.id {
        return false;
    }
    charter_crypto::schnorr_verify(ev.pubkey.as_bytes(), &id, ev.sig.as_bytes())
}

/// Inputs that make a wrap deterministic.
pub struct WrapRandomness {
    pub ephemeral_secret: [u8; 32],
    pub seal_nonce: [u8; 32],
    pub wrap_nonce: [u8; 32],
    pub seal_created_at: u64,
    pub wrap_created_at: u64,
}

/// Seal + wrap a rumor for `recipient_pk`, authored by `author_sk`.
pub fn wrap(
    rumor: &Rumor,
    author_sk: &[u8; 32],
    recipient_pk: &[u8; 32],
    r: &WrapRandomness,
) -> Result<NostrEvent, Nip59Error> {
    // 1. Seal: encrypt the rumor to the recipient with the author's conv key.
    let rumor_json = serde_json::to_string(rumor).map_err(|_| Nip59Error::Json)?;
    let seal_ck =
        nip44::conversation_key_zeroizing(author_sk, recipient_pk).map_err(Nip59Error::Decrypt)?;
    let seal_content =
        nip44::encrypt(&rumor_json, &seal_ck, &r.seal_nonce).map_err(Nip59Error::Decrypt)?;
    let seal = sign_event_raw(
        author_sk,
        kinds::SEAL,
        r.seal_created_at,
        vec![],
        seal_content,
    )?;

    // 2. Wrap: encrypt the seal to the recipient with an ephemeral conv key.
    let seal_json = serde_json::to_string(&seal).map_err(|_| Nip59Error::Json)?;
    let wrap_ck = nip44::conversation_key_zeroizing(&r.ephemeral_secret, recipient_pk)
        .map_err(Nip59Error::Decrypt)?;
    let wrap_content =
        nip44::encrypt(&seal_json, &wrap_ck, &r.wrap_nonce).map_err(Nip59Error::Decrypt)?;
    let tags = vec![vec![
        "p".to_string(),
        PubKey::from_bytes(*recipient_pk).to_hex(),
    ]];
    sign_event_raw(
        &r.ephemeral_secret,
        kinds::GIFT_WRAP,
        r.wrap_created_at,
        tags,
        wrap_content,
    )
}

/// Unwrap a gift-wrap with the recipient's secret. Enforces the wrap/seal kinds
/// + signatures, the author binding, and the jitter bound.
pub fn unwrap(
    wrap: &NostrEvent,
    recipient_sk: &[u8; 32],
    now: u64,
    max_jitter_secs: u64,
) -> Result<Rumor, Nip59Error> {
    unwrap_with_author(wrap, recipient_sk, now, max_jitter_secs).map(|(r, _)| r)
}

/// Like [`unwrap`] but also returns the seal author (the sender that vouched for
/// the rumor). For a guardian-signed grant the seal author equals the inner
/// grant author — both must be in the ward's signer set.
///
/// Strict for every kind: a rumor whose author is not its seal author is
/// refused ([`Nip59Error::AuthorMismatch`]), including the seal-exempt kinds.
/// Those reach a ward only through [`unwrap_opened`].
pub fn unwrap_with_author(
    wrap: &NostrEvent,
    recipient_sk: &[u8; 32],
    now: u64,
    max_jitter_secs: u64,
) -> Result<(Rumor, PubKey), Nip59Error> {
    let (rumor, seal_author) = open(wrap, recipient_sk, now, max_jitter_secs)?;
    // The sealed author must equal the rumor author (no forged sender).
    if rumor.pubkey != seal_author {
        return Err(Nip59Error::AuthorMismatch);
    }
    Ok((rumor, seal_author))
}

/// Inner kinds whose seal author may differ from their signer, so the strict
/// author binding cannot apply to them (contract, *Delivery and the two seal
/// exceptions*):
/// - a guardian statement (31120): an approval is sealed by the working key
///   it names, and a root's statement may be carried by one of its working
///   keys;
/// - a presence confirmation (31121): handed over with both authors so the
///   guardian-keys state machine applies its own rule.
///
/// A PAIR_OFFER is deliberately NOT here: its rumor is authored by the key
/// that seals it, with or without an `approval`, and the unpaired ward pins
/// that seal author, so it keeps the strict binding.
pub const SEAL_EXEMPT_KINDS: [u16; 2] = [
    kinds::CHARTER_GUARDIAN_STATEMENT,
    kinds::CHARTER_PRESENCE_CONFIRM,
];

/// A seal-exempt inner event, with the key that sealed it. Only
/// [`unwrap_opened`] builds one, and only for a [`SEAL_EXEMPT_KINDS`] kind
/// whose own id and signature verified.
///
/// This is the one way an event whose seal author is not its signer reaches
/// a ward, and it can only carry a statement or a presence confirmation, so
/// it can never be passed off as a CLAUSE, GRANT, RELEASE, RELAYS or
/// USAGE_SYNC: their verifiers check the kind first. Its consumer is the
/// guardian-keys state machine, which decides from both authors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardianSealed {
    event: NostrEvent,
    seal_author: PubKey,
}

impl GuardianSealed {
    /// The inner signed event (kind 31120 or 31121), its signature verified.
    pub fn event(&self) -> &NostrEvent {
        &self.event
    }
    /// The inner event's kind.
    pub fn kind(&self) -> u16 {
        self.event.kind
    }
    /// The inner event's author (who signed it).
    pub fn signer(&self) -> PubKey {
        self.event.pubkey
    }
    /// The key that sealed it (authenticated by the seal's signature).
    pub fn seal_author(&self) -> PubKey {
        self.seal_author
    }
    /// The event and its seal author.
    pub fn into_parts(self) -> (NostrEvent, PubKey) {
        (self.event, self.seal_author)
    }

    /// Test fixture: build one directly from a signed event, re-checking
    /// what [`unwrap_opened`] checks (kind and signature).
    #[cfg(any(test, feature = "mock"))]
    pub fn for_test(event: NostrEvent, seal_author: PubKey) -> Option<Self> {
        (SEAL_EXEMPT_KINDS.contains(&event.kind) && sig_valid(&event))
            .then_some(GuardianSealed { event, seal_author })
    }
}

/// What [`unwrap_opened`] found inside a wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// Any kind outside [`SEAL_EXEMPT_KINDS`], with the strict author binding
    /// checked: the rumor and its seal author, which are the same key.
    Bound(Rumor, PubKey),
    /// A seal-exempt kind, for the guardian-keys state machine.
    Guardian(GuardianSealed),
}

/// Unwrap a gift-wrap as [`unwrap_with_author`] does, except that a
/// [`SEAL_EXEMPT_KINDS`] inner event is returned as [`Opened::Guardian`] with
/// both authors instead of being refused when they differ. The inner event
/// must then be a signed event whose own id and signature verify. Every other
/// kind keeps the strict binding.
pub fn unwrap_opened(
    wrap: &NostrEvent,
    recipient_sk: &[u8; 32],
    now: u64,
    max_jitter_secs: u64,
) -> Result<Opened, Nip59Error> {
    let (rumor, seal_author) = open(wrap, recipient_sk, now, max_jitter_secs)?;
    if SEAL_EXEMPT_KINDS.contains(&rumor.kind) {
        let event = rumor.into_signed_event().ok_or(Nip59Error::BadInnerEvent)?;
        if !sig_valid(&event) {
            return Err(Nip59Error::BadInnerEvent);
        }
        return Ok(Opened::Guardian(GuardianSealed { event, seal_author }));
    }
    if rumor.pubkey != seal_author {
        return Err(Nip59Error::AuthorMismatch);
    }
    Ok(Opened::Bound(rumor, seal_author))
}

/// Open a wrap down to the rumor: wrap and seal kinds, signatures and the
/// jitter bound, but NOT the author binding — every public entry point
/// applies that (or, for a seal-exempt kind, hands both authors on).
fn open(
    wrap: &NostrEvent,
    recipient_sk: &[u8; 32],
    now: u64,
    max_jitter_secs: u64,
) -> Result<(Rumor, PubKey), Nip59Error> {
    if wrap.kind != kinds::GIFT_WRAP {
        return Err(Nip59Error::WrongWrapKind);
    }
    // Jitter: wraps are backdated; reject one too far from now in either dir.
    let delta = now.abs_diff(wrap.created_at);
    if delta > max_jitter_secs {
        return Err(Nip59Error::JitterTooLarge);
    }
    if !sig_valid(wrap) {
        return Err(Nip59Error::BadWrapSignature);
    }

    let wrap_ck = nip44::conversation_key_zeroizing(recipient_sk, wrap.pubkey.as_bytes())
        .map_err(Nip59Error::Decrypt)?;
    let seal_json = nip44::decrypt(&wrap.content, &wrap_ck).map_err(Nip59Error::Decrypt)?;
    let seal: NostrEvent = serde_json::from_str(&seal_json).map_err(|_| Nip59Error::Json)?;
    if seal.kind != kinds::SEAL {
        return Err(Nip59Error::WrongSealKind);
    }
    if !sig_valid(&seal) {
        return Err(Nip59Error::BadSealSignature);
    }

    let seal_ck = nip44::conversation_key_zeroizing(recipient_sk, seal.pubkey.as_bytes())
        .map_err(Nip59Error::Decrypt)?;
    let rumor_json = nip44::decrypt(&seal.content, &seal_ck).map_err(Nip59Error::Decrypt)?;
    let rumor: Rumor = serde_json::from_str(&rumor_json).map_err(|_| Nip59Error::Json)?;
    Ok((rumor, seal.pubkey))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(b: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = b;
        s
    }

    #[test]
    fn wrap_unwrap_roundtrip() {
        let author = secret(0x22);
        let recipient = secret(0x11);
        let recipient_pk = charter_crypto::xonly_pubkey(&recipient).unwrap();
        let author_pk = charter_crypto::xonly_pubkey(&author).unwrap();
        let rumor = Rumor {
            id: None,
            pubkey: PubKey::from_bytes(author_pk),
            created_at: 1_700_000_000,
            kind: 31111,
            tags: vec![],
            content: "{\"hi\":1}".into(),
            sig: None,
        };
        let r = WrapRandomness {
            ephemeral_secret: secret(0x55),
            seal_nonce: [1; 32],
            wrap_nonce: [2; 32],
            seal_created_at: 1_700_000_000,
            wrap_created_at: 1_700_000_000,
        };
        let w = wrap(&rumor, &author, &recipient_pk, &r).unwrap();
        let got = unwrap(&w, &recipient, 1_700_000_000, MAX_JITTER_SECS).unwrap();
        assert_eq!(got.pubkey, rumor.pubkey);
        assert_eq!(got.content, rumor.content);
    }

    fn randomness() -> WrapRandomness {
        WrapRandomness {
            ephemeral_secret: secret(0x55),
            seal_nonce: [1; 32],
            wrap_nonce: [2; 32],
            seal_created_at: 1_700_000_000,
            wrap_created_at: 1_700_000_000,
        }
    }

    /// A signed `kind` event by `signer`, sealed by `sealer`, wrapped to
    /// `recipient`.
    fn sealed_by(kind: u16, signer: u8, sealer: u8, recipient: u8) -> NostrEvent {
        let ev = sign_event_raw(
            &secret(signer),
            kind,
            1_700_000_000,
            vec![kinds::marker_tag()],
            "{}".into(),
        )
        .unwrap();
        let recipient_pk = charter_crypto::xonly_pubkey(&secret(recipient)).unwrap();
        wrap(
            &Rumor::from_signed_event(&ev),
            &secret(sealer),
            &recipient_pk,
            &randomness(),
        )
        .unwrap()
    }

    #[test]
    fn a_statement_sealed_by_another_key_opens_only_through_the_seal_exempt_unwrap() {
        let now = 1_700_000_000;
        let w = sealed_by(kinds::CHARTER_GUARDIAN_STATEMENT, 0x21, 0x22, 0x11);
        assert_eq!(
            unwrap_with_author(&w, &secret(0x11), now, MAX_JITTER_SECS),
            Err(Nip59Error::AuthorMismatch),
            "the strict unwrap never hands it on"
        );
        match unwrap_opened(&w, &secret(0x11), now, MAX_JITTER_SECS).unwrap() {
            Opened::Guardian(g) => {
                let signer = charter_crypto::xonly_pubkey(&secret(0x21)).unwrap();
                let sealer = charter_crypto::xonly_pubkey(&secret(0x22)).unwrap();
                assert_eq!(g.signer(), PubKey::from_bytes(signer));
                assert_eq!(g.seal_author(), PubKey::from_bytes(sealer));
                assert_eq!(g.kind(), kinds::CHARTER_GUARDIAN_STATEMENT);
            }
            other => panic!("expected a guardian event, got {other:?}"),
        }
    }

    #[test]
    fn every_other_kind_keeps_the_strict_binding_in_the_seal_exempt_unwrap() {
        let now = 1_700_000_000;
        for kind in [
            kinds::CHARTER_DEVICE_CLAUSE,
            kinds::CHARTER_DEVICE_GRANT,
            kinds::CHARTER_DEVICE_RELEASE,
            kinds::CHARTER_DEVICE_RELAYS,
            kinds::CHARTER_DEVICE_USAGE_SYNC,
            kinds::CHARTER_DEVICE_PAIR_OFFER,
        ] {
            let w = sealed_by(kind, 0x21, 0x22, 0x11);
            assert_eq!(
                unwrap_opened(&w, &secret(0x11), now, MAX_JITTER_SECS),
                Err(Nip59Error::AuthorMismatch),
                "kind {kind}"
            );
            let same = sealed_by(kind, 0x21, 0x21, 0x11);
            assert!(matches!(
                unwrap_opened(&same, &secret(0x11), now, MAX_JITTER_SECS),
                Ok(Opened::Bound(_, _))
            ));
        }
    }

    #[test]
    fn a_seal_exempt_kind_must_be_a_signed_event() {
        let now = 1_700_000_000;
        let author = secret(0x22);
        let rumor = Rumor {
            id: None,
            pubkey: PubKey::from_bytes(charter_crypto::xonly_pubkey(&author).unwrap()),
            created_at: now,
            kind: kinds::CHARTER_GUARDIAN_STATEMENT,
            tags: vec![],
            content: "{}".into(),
            sig: None,
        };
        let recipient_pk = charter_crypto::xonly_pubkey(&secret(0x11)).unwrap();
        let w = wrap(&rumor, &author, &recipient_pk, &randomness()).unwrap();
        assert_eq!(
            unwrap_opened(&w, &secret(0x11), now, MAX_JITTER_SECS),
            Err(Nip59Error::BadInnerEvent)
        );
    }

    #[test]
    fn wrong_recipient_cannot_decrypt() {
        let author = secret(0x22);
        let recipient = secret(0x11);
        let recipient_pk = charter_crypto::xonly_pubkey(&recipient).unwrap();
        let rumor = Rumor {
            id: None,
            pubkey: PubKey::from_bytes(charter_crypto::xonly_pubkey(&author).unwrap()),
            created_at: 1_700_000_000,
            kind: 31111,
            tags: vec![],
            content: "secret".into(),
            sig: None,
        };
        let r = WrapRandomness {
            ephemeral_secret: secret(0x55),
            seal_nonce: [1; 32],
            wrap_nonce: [2; 32],
            seal_created_at: 1_700_000_000,
            wrap_created_at: 1_700_000_000,
        };
        let w = wrap(&rumor, &author, &recipient_pk, &r).unwrap();
        let attacker = secret(0x99);
        assert!(unwrap(&w, &attacker, 1_700_000_000, MAX_JITTER_SECS).is_err());
    }

    #[test]
    fn jitter_bound_enforced() {
        let author = secret(0x22);
        let recipient = secret(0x11);
        let recipient_pk = charter_crypto::xonly_pubkey(&recipient).unwrap();
        let rumor = Rumor {
            id: None,
            pubkey: PubKey::from_bytes(charter_crypto::xonly_pubkey(&author).unwrap()),
            created_at: 1_700_000_000,
            kind: 31111,
            tags: vec![],
            content: "x".into(),
            sig: None,
        };
        let r = WrapRandomness {
            ephemeral_secret: secret(0x55),
            seal_nonce: [1; 32],
            wrap_nonce: [2; 32],
            seal_created_at: 1_700_000_000,
            wrap_created_at: 1_700_000_000,
        };
        let w = wrap(&rumor, &author, &recipient_pk, &r).unwrap();
        // now is a week away — beyond the 2-day jitter bound.
        let far = 1_700_000_000 + 7 * 24 * 3600;
        assert_eq!(
            unwrap(&w, &recipient, far, MAX_JITTER_SECS),
            Err(Nip59Error::JitterTooLarge)
        );
    }
}
