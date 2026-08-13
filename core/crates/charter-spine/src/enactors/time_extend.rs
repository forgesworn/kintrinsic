//! The `time.extend` enactor: on a verified Allow grant it applies a today-only
//! additive extension via the Phase-6 `apply_extension` seam — it does **not**
//! redefine the enforcer math. The guardian may grant less than requested; `0`
//! is a no-op; `>1440` is rejected; the grant `exp` must be today-only (≤ EOD in
//! tz). The child-authored `reason` never reaches this path (it rides only the
//! E2E gift-wrapped request).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use charter_proto::{GrantParams, LimitHit, OpType};
use charter_schedule::Dimension;
use charter_verify::VerifiedGrant;

use crate::enactor::{EnactContext, EnactOutcome, Enactor};
use crate::enforcer_runtime::EnforcerRuntime;
use crate::error::EnactError;
use crate::multi_child::{ExtensionInbox, PendingExtension};

/// Clock-skew tolerance for the today-only expiry check.
const EXTEND_EXP_SKEW: i64 = 300;

/// Enacts `time.extend` by pushing entries to the shared enforcer ledger.
pub struct TimeExtendEnactor {
    enforcer: Arc<Mutex<EnforcerRuntime>>,
    /// When set, each newly-applied extension is ALSO deposited here for the
    /// enforcement loop to drain into the live `MultiChildEnforcer` — the ledger
    /// that actually freezes/unlocks the child. Without it (e.g. unit tests) the
    /// enactor only updates the standalone `EnforcerRuntime`.
    inbox: Option<ExtensionInbox>,
}

impl TimeExtendEnactor {
    pub fn new(enforcer: Arc<Mutex<EnforcerRuntime>>) -> Self {
        TimeExtendEnactor {
            enforcer,
            inbox: None,
        }
    }

    /// Also deposit each applied extension into the shared inbox the enforcement
    /// loop drains into the live `MultiChildEnforcer`, so a verified extension
    /// actually extends/unlocks the enforced child (not just the status readout).
    pub fn with_inbox(mut self, inbox: ExtensionInbox) -> Self {
        self.inbox = Some(inbox);
        self
    }
}

#[async_trait]
impl Enactor for TimeExtendEnactor {
    fn op(&self) -> OpType {
        OpType::TimeExtend
    }

    async fn enact(
        &self,
        grant: &VerifiedGrant,
        ctx: &EnactContext,
    ) -> Result<EnactOutcome, EnactError> {
        let params = match grant.allow_params() {
            Some(GrantParams::TimeExtend(p)) => p.clone(),
            _ => return Err(EnactError::Terminal("not a time.extend allow".into())),
        };
        // `u16` / 1440 cap.
        params
            .validate()
            .map_err(|e| EnactError::Terminal(e.to_string()))?;
        if params.minutes_granted == 0 {
            return Ok(EnactOutcome {
                detail: Some("no effect (0 minutes)".into()),
            });
        }
        let now = ctx
            .now_unix
            .ok_or_else(|| EnactError::Terminal("no clock in context".into()))?;

        // Today-only: the grant must expire by end-of-day in the CLAUSE
        // enforcement tz (the broker computes this via `time_extend_eod`), NOT
        // the daemon's ambient runtime tz — otherwise a clause-tz ≠ runtime-tz
        // mismatch silently rejects a legitimate same-day grant.
        let eod = ctx
            .eod_unix
            .ok_or_else(|| EnactError::Terminal("no enforcement eod in context".into()))?;
        if grant.exp() as i64 > eod + EXTEND_EXP_SKEW {
            return Err(EnactError::Terminal(
                "extension expiry beyond end-of-day".into(),
            ));
        }

        // Per-bucket routing: a Bucket-hit grant credits the named bucket's
        // OWN pool via `ExtensionLedger::apply_bucket`, never the whole-device
        // schedule/budget pool — crediting the wrong one would leave the
        // bucket's apps blocked while the D-Bus readout claims the grant
        // landed. A Bucket grant with no `bucketId` cannot be routed at all,
        // so it fails closed rather than silently landing nowhere.
        if params.limit_hit == LimitHit::Bucket {
            let Some(bucket_id) = params.bucket_id.clone() else {
                return Err(EnactError::Terminal(
                    "bucket-routed time.extend missing bucketId".into(),
                ));
            };
            let applied = {
                let mut e = self.enforcer.lock().expect("enforcer lock");
                e.apply_bucket(
                    now,
                    &grant.req_id().to_hex(),
                    params.minutes_granted,
                    &bucket_id,
                )
            };
            if applied {
                if let Some(inbox) = &self.inbox {
                    inbox.lock().expect("inbox lock").push(PendingExtension {
                        req_id: grant.req_id().to_hex(),
                        minutes: params.minutes_granted,
                        dim: None,
                        bucket_id: Some(bucket_id),
                        at: now,
                    });
                }
            }
            return Ok(EnactOutcome {
                detail: Some(
                    if applied {
                        "extended"
                    } else {
                        "already applied"
                    }
                    .into(),
                ),
            });
        }
        let dim = match params.limit_hit {
            LimitHit::Schedule => Dimension::Schedule,
            LimitHit::Budget => Dimension::Budget,
            // Unreachable in practice — the Bucket arm above always returns —
            // but this is a privileged daemon shared with the Android JNI
            // warden, so a dead branch must fail SOFT rather than panic the
            // whole process. Same shape as the "missing bucketId" case above:
            // a Terminal error the broker surfaces/logs, never a crash.
            LimitHit::Bucket => {
                return Err(EnactError::Terminal(
                    "time.extend: bucket-hit reached whole-device routing unexpectedly".into(),
                ))
            }
        };
        // Idempotent by reqId — a re-delivered grant does not double-extend.
        let applied = {
            let mut e = self.enforcer.lock().expect("enforcer lock");
            e.apply_extension(now, &grant.req_id().to_hex(), params.minutes_granted, dim)
        };
        // Newly applied: hand it to the enforcement loop's live per-child ledger
        // (the EnforcerRuntime above drives only the D-Bus readout, not freeze).
        if applied {
            if let Some(inbox) = &self.inbox {
                inbox.lock().expect("inbox lock").push(PendingExtension {
                    req_id: grant.req_id().to_hex(),
                    minutes: params.minutes_granted,
                    dim: Some(dim),
                    bucket_id: None,
                    at: now,
                });
            }
        }
        Ok(EnactOutcome {
            detail: Some(
                if applied {
                    "extended"
                } else {
                    "already applied"
                }
                .into(),
            ),
        })
    }
}
