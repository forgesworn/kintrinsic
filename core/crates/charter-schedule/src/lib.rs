//! `charter-schedule` — the schedule + budget evaluator and the enforcer
//! decision engine. Pure logic over an injected clock; no IO. The runtime
//! evaluators are the device authority; the SDK-parity `evaluate_schedule` is a
//! cross-check shim only.

pub mod buckets;
pub mod budget;
pub mod clause;
pub mod enforcer;
pub mod extension;
pub mod minutes;
pub mod schedule_eval;
pub mod tethering;
pub mod usage;

pub use buckets::{
    bucket_day_remaining_secs, bucket_for_app, bucket_remaining_secs, bucket_week_remaining_secs,
    spent_bucket_apps, BucketSpent,
};
pub use budget::{quota_left, quota_left_pooled, ConsolidatedUsage, QuotaStatus};
pub use clause::{
    cmdline_needle, is_cmdline_id, is_site_id, site_id, AppBucket, AppRule, ChartedClause,
    GrantAppRules, GrantBuckets, GrantBudget, GrantSchedule, GrantScheduleWindow, ScheduleWindow,
    TimeModel, WeekStart, WeeklySchedule, APP_RULES_VERSION, BUCKETS_VERSION,
    BUCKETS_VERSION_WEEKLY, CMDLINE_ID_PREFIX, MAX_BUCKET_WEEK_MINUTES, MIN_CMDLINE_NEEDLE,
    SITE_ID_PREFIX,
};
pub use enforcer::{
    burn_schedule_extension, compute_remaining, end_of_day_unix, enforcement_eod_unix,
    enforcement_tz_of, is_valid_freeze_target, reconcile_freeze, AuditKind, EnforcerCore,
    EnforcerEffect, EnforcerInputs, FreezeAction, FreezeTarget, LockReason, Remaining, StandDown,
    WarnLevel,
};
pub use extension::{day_key, Dimension, ExtensionLedger};
pub use minutes::MinuteSet;
pub use schedule_eval::{
    evaluate_app_rule, evaluate_app_rules, evaluate_grant_schedule, evaluate_schedule, AppAccess,
    EvaluateReason, EvaluateResult, ScheduleStatus,
};
pub use tethering::{evaluate_tethering, GrantTethering, TetherAllow, TetherMode};
pub use usage::{Activity, Bucket, UsageLedger};
