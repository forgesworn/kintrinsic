//! `time.extend` (Flow C 3-5): a verified Allow grant applies a today-only
//! additive extension via the Phase-6 seam, thawing when `effective > 0`. The
//! enactor only pushes ledger entries — the enforcer math is unchanged. The
//! child-authored `reason` never reaches the grant/enactor (privacy).

#![cfg(feature = "mock")]

use std::sync::{Arc, Mutex};

use charter_primitives::{Nonce, ReqId};
use charter_proto::OpType;
use charter_schedule::{end_of_day_unix, Activity, GrantBudget, WeekStart};
use charter_sys::persistence::ClauseStore;
use charter_sys::{MockSystem, SystemLayer};
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charter_verify::{verify_grant, VerifiedGrant, VerifyParams};
use charterd::enactor::{EnactContext, Enactor};
use charterd::enactors::time_extend::TimeExtendEnactor;
use charterd::enforcer_runtime::EnforcerRuntime;
use charterd::error::EnactError;

const TZ: &str = "Europe/London";
const IN_WINDOW: i64 = 1_782_752_400; // Mon 18:00 BST
const MANAGED_UID: u32 = 1000;

fn rid() -> ReqId {
    ReqId::from_bytes([0xA1; 32])
}
fn non() -> Nonce {
    Nonce::from_bytes([0xB2; 32])
}

fn eod() -> i64 {
    end_of_day_unix(TZ, IN_WINDOW)
}

fn budget_json(daily: u32) -> String {
    serde_json::to_string(&GrantBudget {
        model: None,
        v: 1,
        tz: TZ.into(),
        daily_minutes: Some(daily),
        weekly_minutes: None,
        week_start: Some(WeekStart::Mon),
        paused: None,
        revoked: None,
        issued_at: 100,
    })
    .unwrap()
}

fn build_extend_grant(req: ReqId, params: serde_json::Value, exp: i64) -> VerifiedGrant {
    try_build_extend_grant(req, params, exp).unwrap()
}

fn try_build_extend_grant(
    req: ReqId,
    params: serde_json::Value,
    exp: i64,
) -> Result<VerifiedGrant, charter_verify::VerifyError> {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(req, non())
        .op(OpType::TimeExtend)
        .params(params)
        .ts((IN_WINDOW - 10) as u64)
        .exp(exp as u64)
        .build(&g);
    let pk = g.pubkey();
    let store = charter_sys::persistence::MockConsumedIdStore::new(
        charter_sys::persistence::MockDisk::new(),
    );
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &req,
        expected_nonce: &non(),
        expected_op: OpType::TimeExtend,
        now: IN_WINDOW as u64,
    };
    verify_grant(&ev, &p, &store)
}

/// A verified time.extend grant with explicit reqId, minutes, dimension, exp.
fn extend_grant(req: ReqId, minutes: u16, dim: &str, exp: i64) -> VerifiedGrant {
    build_extend_grant(
        req,
        serde_json::json!({"minutesGranted": minutes, "limitHit": dim}),
        exp,
    )
}

/// A verified per-group (`limitHit: "bucket"`) time.extend grant, optionally
/// carrying `bucketId` — `None` reproduces a malformed/legacy grant with no
/// bucket to route to.
fn extend_bucket_grant(
    req: ReqId,
    minutes: u16,
    bucket_id: Option<&str>,
    exp: i64,
) -> VerifiedGrant {
    let params = match bucket_id {
        Some(id) => {
            serde_json::json!({"minutesGranted": minutes, "limitHit": "bucket", "bucketId": id})
        }
        None => serde_json::json!({"minutesGranted": minutes, "limitHit": "bucket"}),
    };
    build_extend_grant(req, params, exp)
}

fn ctx() -> EnactContext {
    EnactContext {
        source_path: None,
        now_unix: Some(IN_WINDOW),
        // The broker supplies the clause-tz end-of-day; here clause tz == TZ.
        eod_unix: Some(eod()),
    }
}

/// Decide (sync, lock released) + apply the async port effects. The freeze slice
/// is resolved caller-side from the managed uid (never held across the await).
async fn tick_enf(enf: &Arc<Mutex<EnforcerRuntime>>, sys: &MockSystem, a: Activity, e: u64) {
    let effs = {
        let mut g = enf.lock().unwrap();
        g.tick(sys, a, e)
    };
    let slice = charterd::enforcer_runtime::managed_freeze_target(MANAGED_UID);
    charterd::enforcer_runtime::apply_effects(sys, &effs, &slice).await;
}

/// A system locked on a 120-min budget + a shared enforcer (already locked).
async fn locked_setup() -> (MockSystem, Arc<Mutex<EnforcerRuntime>>) {
    let sys = MockSystem::new(IN_WINDOW as u64);
    sys.clauses().put_clause(2, 100, &budget_json(120)).unwrap();
    let enforcer = Arc::new(Mutex::new(EnforcerRuntime::fresh(TZ, IN_WINDOW)));
    tick_enf(&enforcer, &sys, Activity::Active, 120 * 60).await; // spend quota -> lock
    assert!(enforcer.lock().unwrap().is_locked());
    (sys, enforcer)
}

#[tokio::test]
async fn budget_extension_unlocks() {
    let (sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer.clone());
    enactor
        .enact(&extend_grant(rid(), 15, "budget", eod()), &ctx())
        .await
        .unwrap();
    tick_enf(&enforcer, &sys, Activity::Idle, 0).await;
    assert!(
        !enforcer.lock().unwrap().is_locked(),
        "budget extension thaws"
    );
}

#[tokio::test]
async fn wrong_dimension_does_not_unlock() {
    // Locked on budget; a SCHEDULE extension must not unlock it.
    let (sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer.clone());
    enactor
        .enact(&extend_grant(rid(), 60, "schedule", eod()), &ctx())
        .await
        .unwrap();
    tick_enf(&enforcer, &sys, Activity::Idle, 0).await;
    assert!(
        enforcer.lock().unwrap().is_locked(),
        "wrong-dimension extension keeps the lock"
    );
}

#[tokio::test]
async fn idempotent_by_reqid() {
    let (_sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer.clone());
    let grant = extend_grant(rid(), 15, "budget", eod());
    let first = enactor.enact(&grant, &ctx()).await.unwrap();
    let second = enactor.enact(&grant, &ctx()).await.unwrap();
    assert_eq!(first.detail.as_deref(), Some("extended"));
    assert_eq!(
        second.detail.as_deref(),
        Some("already applied"),
        "no double extension"
    );
}

#[tokio::test]
async fn zero_minutes_is_noop() {
    let (_sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer);
    let out = enactor
        .enact(&extend_grant(rid(), 0, "budget", eod()), &ctx())
        .await
        .unwrap();
    assert_eq!(out.detail.as_deref(), Some("no effect (0 minutes)"));
}

#[tokio::test]
async fn expiry_beyond_eod_rejected() {
    let (_sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer);
    // exp one day past EOD -> not today-only -> terminal.
    let grant = extend_grant(rid(), 15, "budget", eod() + 24 * 3600);
    let err = enactor.enact(&grant, &ctx()).await.unwrap_err();
    assert!(matches!(err, EnactError::Terminal(_)));
}

/// Over the contract's 1440-minute cap no longer even VERIFIES:
/// `GrantParams::parse` validates at the trust boundary, so no reader of
/// `allow_params()` can ever see the unbounded value. (The enactor keeps its
/// own check as defence in depth.)
#[test]
fn over_max_minutes_never_becomes_a_verified_grant() {
    let params = |m: u32| serde_json::json!({"minutesGranted": m, "limitHit": "budget"});
    assert!(try_build_extend_grant(rid(), params(2000), eod()).is_err()); // > 1440
    assert!(try_build_extend_grant(rid(), params(1440), eod()).is_ok());
}

/// A per-group (`limitHit: bucket`) grant credits the NAMED BUCKET's own
/// pool — never the whole-device schedule/budget dimension — via
/// `EnforcerRuntime::apply_bucket`, and (when wired with an inbox, as the
/// real daemon always is) deposits a `bucket_id`-carrying `PendingExtension`
/// for the enforcement loop to drain into the live `MultiChildEnforcer`.
#[tokio::test]
async fn bucket_extension_credits_the_named_pool_not_the_device_dimension() {
    let (sys, enforcer) = locked_setup().await;
    let inbox: charterd::multi_child::ExtensionInbox = Arc::new(Mutex::new(Vec::new()));
    let enactor = TimeExtendEnactor::new(enforcer.clone()).with_inbox(inbox.clone());
    let out = enactor
        .enact(&extend_bucket_grant(rid(), 15, Some("play"), eod()), &ctx())
        .await
        .unwrap();
    assert_eq!(out.detail.as_deref(), Some("extended"));

    {
        let pending = inbox.lock().unwrap();
        assert_eq!(
            pending.len(),
            1,
            "one entry deposited for the loop to drain"
        );
        assert_eq!(pending[0].bucket_id.as_deref(), Some("play"));
        assert_eq!(pending[0].minutes, 15);
        assert!(
            pending[0].dim.is_none(),
            "a bucket-routed entry carries no whole-device dimension"
        );
    }

    // The budget dimension the child was ALREADY locked on (see
    // `locked_setup`) must be untouched by a bucket-routed grant — it credits
    // a different pool entirely.
    tick_enf(&enforcer, &sys, Activity::Idle, 0).await;
    assert!(
        enforcer.lock().unwrap().is_locked(),
        "a bucket grant must not unlock the whole-device budget wall"
    );
}

/// Idempotent by reqId, exactly like the whole-device dimensions.
#[tokio::test]
async fn bucket_extension_idempotent_by_reqid() {
    let (_sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer);
    let grant = extend_bucket_grant(rid(), 15, Some("play"), eod());
    let first = enactor.enact(&grant, &ctx()).await.unwrap();
    let second = enactor.enact(&grant, &ctx()).await.unwrap();
    assert_eq!(first.detail.as_deref(), Some("extended"));
    assert_eq!(
        second.detail.as_deref(),
        Some("already applied"),
        "no double extension"
    );
}

/// A Bucket-hit grant with no `bucketId` cannot be routed anywhere — it must
/// fail closed rather than silently crediting the wrong pool (or none).
#[tokio::test]
async fn bucket_extension_without_a_bucket_id_is_terminal() {
    let (_sys, enforcer) = locked_setup().await;
    let enactor = TimeExtendEnactor::new(enforcer);
    let grant = extend_bucket_grant(rid(), 15, None, eod());
    let err = enactor.enact(&grant, &ctx()).await.unwrap_err();
    assert!(matches!(err, EnactError::Terminal(_)));
}
