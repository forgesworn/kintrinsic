//! The enactor seam. An enactor turns an allow [`VerifiedGrant`] into an OS
//! effect. An unregistered op is **fail-closed** (`NoEnactor`) — never a panic,
//! never an implicit allow.

use async_trait::async_trait;

use charter_proto::OpType;
use charter_verify::VerifiedGrant;

use crate::error::EnactError;

/// The result of a successful enact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnactOutcome {
    pub detail: Option<String>,
}

/// Per-request context the broker hands to an enactor (e.g. the `exec.allow`
/// reqId->source_path binding, the current clock for the `time.extend`
/// today-only check). The grant itself stays the sole authority for *what* to
/// enact; this carries only the local bindings needed to act.
#[derive(Debug, Clone, Default)]
pub struct EnactContext {
    pub source_path: Option<String>,
    pub now_unix: Option<i64>,
    /// For `time.extend`: end-of-day unix in the **clause** enforcement tz (the
    /// broker computes this so the today-only check matches `compute_remaining`,
    /// not the daemon's ambient runtime tz).
    pub eod_unix: Option<i64>,
}

/// Turns an allow grant into an OS effect, acting on the **signed grant params**.
#[async_trait]
pub trait Enactor: Send + Sync {
    /// Which op this enactor handles.
    fn op(&self) -> OpType;
    /// Enact the grant. The grant is guaranteed verified + allow by the broker.
    async fn enact(
        &self,
        grant: &VerifiedGrant,
        ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError>;
}

/// A registry dispatching grants to the matching enactor.
#[derive(Default)]
pub struct EnactorRegistry {
    enactors: Vec<Box<dyn Enactor>>,
}

impl EnactorRegistry {
    pub fn new() -> Self {
        EnactorRegistry {
            enactors: Vec::new(),
        }
    }

    /// Register an enactor (last registration for an op wins).
    pub fn register(&mut self, enactor: Box<dyn Enactor>) {
        self.enactors.push(enactor);
    }

    /// Dispatch to the enactor for the grant's op. Fail-closed if none.
    pub async fn enact(
        &self,
        grant: &VerifiedGrant,
        ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        match self.enactors.iter().rev().find(|e| e.op() == grant.op()) {
            Some(e) => e.enact(grant, ctx).await,
            None => Err(EnactError::NoEnactor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::{Nonce, ReqId};
    use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
    use charter_verify::test_support::{GrantBuilder, TestGuardian};
    use charter_verify::{verify_grant, VerifyParams};

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

    #[tokio::test]
    async fn unregistered_op_is_fail_closed() {
        let reg = EnactorRegistry::new();
        assert_eq!(
            reg.enact(&allow_grant(), &EnactContext::default()).await,
            Err(EnactError::NoEnactor)
        );
    }
}
