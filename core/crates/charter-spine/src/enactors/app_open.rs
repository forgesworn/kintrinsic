//! The `app.open` enactor: a **no-op**, deliberately. The grant carries no
//! enactable effect of its own (see `charter_proto::AppOpenGrantParams`) — the
//! app the ward asked about is actually opened by a SEPARATE, guardian-signed
//! `apps` clause carrying an `AppHold`, published alongside this grant. What
//! this enactor exists for is purely the lifecycle machinery: an op with NO
//! registered enactor is fail-closed (`EnactError::NoEnactor`, terminal —
//! see `EnactorRegistry::enact`), which would strand every allowed app.open
//! ask in `Enacting` -> `Failed` even after `AppOpenGrantParams` let it verify
//! at all. Registering this turns that into the same `Enacted` terminal state
//! `time.extend`/`install.*` reach, which is what the ward's own ask-lifecycle
//! UI (Android `AskLifecycle`/`GroupMirror`, the Linux tray) actually renders.

use async_trait::async_trait;

use charter_proto::{GrantParams, OpType};
use charter_verify::VerifiedGrant;

use crate::enactor::{EnactContext, EnactOutcome, Enactor};
use crate::error::EnactError;

/// Enacts `app.open` by doing nothing — see the module doc.
pub struct AppOpenEnactor;

#[async_trait]
impl Enactor for AppOpenEnactor {
    fn op(&self) -> OpType {
        OpType::AppOpen
    }

    async fn enact(
        &self,
        grant: &VerifiedGrant,
        _ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        match grant.allow_params() {
            Some(GrantParams::AppOpen(_)) => Ok(EnactOutcome {
                detail: Some("no enact — the apps clause carries the hold".into()),
            }),
            _ => Err(EnactError::Terminal("not an app.open allow".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::{Nonce, ReqId};
    use charter_proto::AppOpenGrantParams;
    use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
    use charter_verify::test_support::{GrantBuilder, TestGuardian};
    use charter_verify::{verify_grant, VerifyParams};

    fn verified_allow() -> VerifiedGrant {
        let g = TestGuardian::new();
        let rid = ReqId::from_bytes([9; 32]);
        let non = Nonce::from_bytes([8; 32]);
        let params = AppOpenGrantParams {
            pkg: "com.mojang.minecraftpe".into(),
            minutes_granted: 30,
        };
        let ev = GrantBuilder::app_open_allow(rid, non, serde_json::to_value(&params).unwrap())
            .build(&g);
        let pk = g.pubkey();
        let store = MockConsumedIdStore::new(MockDisk::new());
        let p = VerifyParams {
            pinned_guardian: &pk,
            expected_req_id: &rid,
            expected_nonce: &non,
            expected_op: OpType::AppOpen,
            now: 1_700_001_000,
        };
        verify_grant(&ev, &p, &store).unwrap()
    }

    #[tokio::test]
    async fn enacting_an_app_open_allow_is_a_no_op_success() {
        let out = AppOpenEnactor
            .enact(&verified_allow(), &EnactContext::default())
            .await
            .unwrap();
        assert!(out.detail.is_some());
    }

    #[tokio::test]
    async fn registers_under_the_app_open_op() {
        assert_eq!(AppOpenEnactor.op(), OpType::AppOpen);
    }
}
