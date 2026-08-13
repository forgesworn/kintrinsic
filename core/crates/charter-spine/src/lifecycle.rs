//! The pure, fail-closed lifecycle reducer. **`Enact` is producible only from a
//! `GrantOutcome::Allow`** — a rejected or absent grant can never yield an
//! enact effect. Zero IO; fully unit-tested.

use serde::{Deserialize, Serialize};

use charter_primitives::{Nonce, ReqId};
use charter_proto::OpType;
use charter_verify::{GrantOutcome, VerifiedGrant, VerifyError};

use crate::audit::{AuditOutcome, AuditRecord};

/// Lifecycle state of a brokered request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    Enacting,
    Enacted,
    Denied,
    Failed,
    Rejected,
    Expired,
    Cancelled,
}

impl RequestState {
    /// Whether this is a terminal state (no further transitions).
    pub fn is_terminal(&self) -> bool {
        !matches!(self, RequestState::Pending | RequestState::Enacting)
    }
}

/// A persisted brokered-request record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub req_id: ReqId,
    pub nonce: Nonce,
    pub op: OpType,
    pub state: RequestState,
    pub created_at: u64,
    pub detail: Option<String>,
    /// For `exec.allow`: the managed-tree source path bound to this reqId
    /// (the hash never leaves charterd; the path is needed to relocate bytes).
    pub source_path: Option<String>,
    /// The KERNEL-attested uid that submitted this request, on platforms with
    /// more than one user (S8, review 2026-08-07). Never read from a request
    /// body — see `dbus_service`'s `caller_uid`.
    ///
    /// Whose request this is, so a shared family laptop can answer "show me my
    /// asks" without showing a child every sibling's. `None` means unowned:
    /// a single-user platform (Android) where the question does not arise, or
    /// a record persisted before this field existed. An unowned record is
    /// visible to root only — the fail-closed reading — which on Linux costs
    /// at most the in-flight asks of one upgrade, and self-heals as those are
    /// answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_uid: Option<u32>,
}

impl RequestRecord {
    /// Whether `uid` may see or withdraw this request.
    ///
    /// Root is exempt: charterd itself runs as root, and an administrator at
    /// the machine is already the authority every other path defers to.
    /// Otherwise ownership is exact — and an UNOWNED record belongs to nobody,
    /// so it is never handed to a non-root caller on the strength of a missing
    /// field.
    pub fn visible_to(&self, uid: u32) -> bool {
        uid == 0 || self.caller_uid == Some(uid)
    }
}

/// Events that drive a record through its lifecycle.
pub enum Event {
    /// A grant that passed every verification rule (allow or deny).
    GrantVerified(VerifiedGrant),
    /// A delivered grant that failed verification (fail-closed: never enacts).
    GrantRejected(VerifyError),
    /// The enactor succeeded.
    EnactSucceeded,
    /// The enactor failed (terminal or transient).
    EnactFailed(String),
    /// The owner withdrew a pending request.
    Cancel,
    /// The pending request's TTL expired.
    Expire,
}

/// Side effects the broker must carry out. **`Enact` carries the verified grant
/// and is produced ONLY from an allow decision.**
pub enum Effect {
    Enact(VerifiedGrant),
    Audit(AuditRecord),
    Notify,
    Persist,
}

/// The pure transition. Returns the next state + the effects to run.
pub fn transition(rec: &RequestRecord, ev: Event) -> (RequestState, Vec<Effect>) {
    match (rec.state, ev) {
        // A verified grant: allow -> enact; deny -> denied (NEVER enacts).
        (RequestState::Pending, Event::GrantVerified(vg)) => match vg.outcome() {
            GrantOutcome::Allow(_) => (
                RequestState::Enacting,
                // Notify so the UI reflects "enacting"; Persist so a crash
                // mid-enact leaves a durable Enacting record (M9 reconciles it).
                vec![Effect::Enact(vg), Effect::Notify, Effect::Persist],
            ),
            GrantOutcome::Deny => (
                RequestState::Denied,
                vec![
                    Effect::Audit(AuditRecord::new(AuditOutcome::Denied, rec.op)),
                    Effect::Notify,
                    Effect::Persist,
                ],
            ),
        },
        // M6: an UNauthenticated verify failure (forged / replayed / malformed
        // grant) must NOT terminally reject the request — a hostile relay could
        // otherwise strand it by racing a forged grant ahead of the guardian's
        // real one. Stay Pending and ignore; only an authenticated Deny or a TTL
        // Expire is terminal. A forged grant never consumes the reqId (the
        // signature check precedes consume), so the genuine grant can still land.
        (RequestState::Pending, Event::GrantRejected(_)) => (RequestState::Pending, vec![]),
        (RequestState::Pending, Event::Cancel) => (RequestState::Cancelled, vec![Effect::Persist]),
        (RequestState::Pending, Event::Expire) => {
            (RequestState::Expired, vec![Effect::Notify, Effect::Persist])
        }

        // Enact outcomes.
        (RequestState::Enacting, Event::EnactSucceeded) => (
            RequestState::Enacted,
            vec![
                Effect::Audit(AuditRecord::new(AuditOutcome::Enacted, rec.op)),
                Effect::Notify,
                Effect::Persist,
            ],
        ),
        (RequestState::Enacting, Event::EnactFailed(_)) => (
            RequestState::Failed,
            vec![
                Effect::Audit(AuditRecord::new(AuditOutcome::Failed, rec.op)),
                Effect::Notify,
                Effect::Persist,
            ],
        ),

        // Any other event in any other state is a no-op (terminal states stick).
        (state, _) => (state, vec![]),
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use charter_primitives::{Nonce, ReqId};
    use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
    use charter_verify::test_support::{GrantBuilder, TestGuardian};
    use charter_verify::{verify_grant, VerifyParams};

    fn rec() -> RequestRecord {
        RequestRecord {
            req_id: ReqId::from_bytes([1; 32]),
            nonce: Nonce::from_bytes([2; 32]),
            op: OpType::InstallFlatpak,
            state: RequestState::Pending,
            created_at: 0,
            detail: None,
            source_path: None,
            caller_uid: None,
        }
    }

    fn allow_grant() -> VerifiedGrant {
        let g = TestGuardian::new();
        let rid = ReqId::from_bytes([1; 32]);
        let non = Nonce::from_bytes([2; 32]);
        let ev = GrantBuilder::install_allow(rid, non).build(&g);
        let pk = g.pubkey();
        let store = MockConsumedIdStore::new(MockDisk::new());
        let p = VerifyParams {
            pinned_guardian: &pk,
            expected_req_id: &rid,
            expected_nonce: &non,
            expected_op: OpType::InstallFlatpak,
            now: 1_700_001_000,
        };
        verify_grant(&ev, &p, &store).unwrap()
    }

    fn deny_grant() -> VerifiedGrant {
        let g = TestGuardian::new();
        let rid = ReqId::from_bytes([1; 32]);
        let non = Nonce::from_bytes([2; 32]);
        let ev = GrantBuilder::install_allow(rid, non).deny().build(&g);
        let pk = g.pubkey();
        let store = MockConsumedIdStore::new(MockDisk::new());
        let p = VerifyParams {
            pinned_guardian: &pk,
            expected_req_id: &rid,
            expected_nonce: &non,
            expected_op: OpType::InstallFlatpak,
            now: 1_700_001_000,
        };
        verify_grant(&ev, &p, &store).unwrap()
    }

    fn has_enact(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::Enact(_)))
    }

    #[test]
    fn allow_grant_yields_enact() {
        let (state, effects) = transition(&rec(), Event::GrantVerified(allow_grant()));
        assert_eq!(state, RequestState::Enacting);
        assert!(has_enact(&effects));
    }

    #[test]
    fn deny_grant_never_enacts() {
        let (state, effects) = transition(&rec(), Event::GrantVerified(deny_grant()));
        assert_eq!(state, RequestState::Denied);
        assert!(
            !has_enact(&effects),
            "a deny grant must never produce an enact"
        );
    }

    #[test]
    fn unauthenticated_grant_stays_pending_never_enacts() {
        // M6: a forged/unauthenticated grant must not strand the request — it
        // stays Pending (so the guardian's real grant can still land) and never
        // enacts. (Liveness defense against a hostile relay.)
        let (state, effects) =
            transition(&rec(), Event::GrantRejected(VerifyError::UntrustedSigner));
        assert_eq!(state, RequestState::Pending);
        assert!(!has_enact(&effects));
        assert!(effects.is_empty(), "a forged grant emits nothing");
    }

    #[test]
    fn enact_success_then_audit() {
        let mut r = rec();
        r.state = RequestState::Enacting;
        let (state, effects) = transition(&r, Event::EnactSucceeded);
        assert_eq!(state, RequestState::Enacted);
        assert!(effects.iter().any(|e| matches!(e, Effect::Audit(_))));
    }

    #[test]
    fn terminal_state_is_inert() {
        let mut r = rec();
        r.state = RequestState::Enacted;
        let (state, effects) = transition(&r, Event::Cancel);
        assert_eq!(state, RequestState::Enacted);
        assert!(effects.is_empty());
    }
}
