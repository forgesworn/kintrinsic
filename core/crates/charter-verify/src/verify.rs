//! `verify_grant` — the six §5 grant-verification rules in fixed order. A
//! [`VerifiedGrant`] has **no public constructor**: the only way to obtain one
//! is to pass every rule, which encodes "no local authority" — an enactor
//! cannot act without a guardian-signed, fresh, single-use, pinned-authority,
//! request-bound grant.

use charter_primitives::{Nonce, NostrEvent, PubKey, ReqId};
use charter_proto::{Decision, GrantParams, GrantPayload, OpType};
use charter_sys::persistence::ConsumedIdStore;
use charter_sys::SysError;

use crate::event::{event_id, signature_is_valid};

/// Clock-skew tolerance for freshness checks (seconds).
pub const FRESHNESS_SKEW_SECS: u64 = 300;

/// Inputs that bind a grant to a known pending request. **No request params** —
/// the enactor acts on the params inside the signed grant, never the request.
pub struct VerifyParams<'a> {
    /// The guardian pubkey pinned at pairing (the only authority).
    pub pinned_guardian: &'a PubKey,
    /// The reqId of the pending request this grant must answer.
    pub expected_req_id: &'a ReqId,
    /// The nonce of the pending request.
    pub expected_nonce: &'a Nonce,
    /// The op of the pending request this grant must answer. A grant whose `op`
    /// differs is rejected ([`VerifyError::OpMismatch`]) so a guardian-side reqId
    /// reuse/misroute across two in-flight dialogs can never enact under the
    /// wrong op with the original request's bindings, nor mislabel the audit —
    /// the enacted op then provably equals the pending record's op.
    pub expected_op: OpType,
    /// Current wall-clock unix seconds.
    pub now: u64,
}

/// The verified outcome — either an allow carrying the approved params, or a
/// deny. Both are authentic guardian decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantOutcome {
    Allow(GrantParams),
    Deny,
}

/// A grant that has passed every verification rule. Fields are private; only
/// [`verify_grant`] constructs it (no public constructor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGrant {
    op: OpType,
    req_id: ReqId,
    exp: u64,
    outcome: GrantOutcome,
}

impl VerifiedGrant {
    /// The op this grant decides.
    pub fn op(&self) -> OpType {
        self.op
    }
    /// The bound request id.
    pub fn req_id(&self) -> &ReqId {
        &self.req_id
    }
    /// Grant expiry (unix seconds).
    pub fn exp(&self) -> u64 {
        self.exp
    }
    /// The verified outcome (allow-with-params or deny).
    pub fn outcome(&self) -> &GrantOutcome {
        &self.outcome
    }
    /// Convenience: the approved params if this is an allow.
    pub fn allow_params(&self) -> Option<&GrantParams> {
        match &self.outcome {
            GrantOutcome::Allow(p) => Some(p),
            GrantOutcome::Deny => None,
        }
    }
}

/// Why a grant was rejected. Every `Err` is fail-CLOSED — no effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// Content/params did not parse, or the event kind was wrong.
    Malformed,
    /// The claimed event id did not match the content.
    EventIdMismatch,
    /// The schnorr signature did not verify.
    BadSignature,
    /// The author is not the pinned guardian.
    UntrustedSigner,
    /// The grant's op did not match the pending request's op (confused-deputy /
    /// audit-accuracy defense-in-depth — the grant answers a different op than
    /// the one this reqId is pending on).
    OpMismatch,
    /// The grant's reqId did not echo the pending request.
    ReqIdMismatch,
    /// The grant's nonce did not echo the pending request.
    NonceMismatch,
    /// The grant has expired.
    Expired,
    /// The grant is dated too far in the future.
    NotYetValid,
    /// `exp <= ts` (degenerate expiry).
    BadExpiry,
    /// The reqId was already consumed (replay).
    Replayed,
    /// The consumed-id store failed.
    Store(SysError),
}

/// Verify a GRANT event against the pinned guardian + pending request, in fixed
/// order: malformed → event-id integrity → signature → pinned authority →
/// op+reqId+nonce binding → freshness/exp → single-use (consume BEFORE returning).
///
/// On success the reqId is durably consumed *before* the grant is returned, so
/// the caller can never enact a grant it has not single-use-claimed.
pub fn verify_grant(
    event: &NostrEvent,
    p: &VerifyParams,
    store: &dyn ConsumedIdStore,
) -> Result<VerifiedGrant, VerifyError> {
    // 1. Malformed: wrong kind, unparseable payload, or (for allow) unparseable
    //    params. Parsing params up-front means a malformed grant never consumes.
    if event.kind != charter_primitives::kinds::CHARTER_DEVICE_GRANT {
        return Err(VerifyError::Malformed);
    }
    let payload = GrantPayload::from_json(&event.content).map_err(|_| VerifyError::Malformed)?;
    let outcome = match payload.decision {
        Decision::Allow => {
            let params = payload.grant_params().map_err(|_| VerifyError::Malformed)?;
            GrantOutcome::Allow(params)
        }
        Decision::Deny => GrantOutcome::Deny,
    };

    // 2. Event-id integrity.
    if event_id(event) != event.id {
        return Err(VerifyError::EventIdMismatch);
    }

    // 3. Signature.
    if !signature_is_valid(event) {
        return Err(VerifyError::BadSignature);
    }

    // 4. Pinned authority.
    if &event.pubkey != p.pinned_guardian {
        return Err(VerifyError::UntrustedSigner);
    }

    // 5. Request binding (op + reqId + nonce echoed verbatim). Binding the op —
    //    not just the reqId, which the caller uses to *find* the pending record
    //    (making the reqId echo near-tautological) — stops a reqId reuse/misroute
    //    from enacting under a different op than the pending request authorized,
    //    and keeps the audit label bound to what is actually enacted. Checked
    //    before single-use consume (step 7) so a mismatch never burns the reqId.
    if payload.op != p.expected_op {
        return Err(VerifyError::OpMismatch);
    }
    if &payload.req_id != p.expected_req_id {
        return Err(VerifyError::ReqIdMismatch);
    }
    if &payload.nonce != p.expected_nonce {
        return Err(VerifyError::NonceMismatch);
    }

    // 6. Freshness / expiry (skew-tolerant).
    if payload.exp <= payload.ts {
        return Err(VerifyError::BadExpiry);
    }
    if p.now + FRESHNESS_SKEW_SECS < payload.ts {
        return Err(VerifyError::NotYetValid);
    }
    if p.now > payload.exp + FRESHNESS_SKEW_SECS {
        return Err(VerifyError::Expired);
    }

    // 7. Single-use: consume BEFORE returning the grant. Retain the reqId until
    //    exp + SKEW — the SAME window freshness accepts (step 6) — so a replay
    //    arriving in (exp, exp + SKEW] cannot slip past a purge-at-exp (M4).
    let newly = store
        .check_and_consume(
            &payload.req_id,
            payload.exp.saturating_add(FRESHNESS_SKEW_SECS),
        )
        .map_err(VerifyError::Store)?;
    if !newly {
        return Err(VerifyError::Replayed);
    }

    Ok(VerifiedGrant {
        op: payload.op,
        req_id: payload.req_id,
        exp: payload.exp,
        outcome,
    })
}
