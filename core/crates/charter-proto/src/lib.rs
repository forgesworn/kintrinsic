//! `charter-proto` — the pure REQUEST / GRANT / CLAUSE wire payloads, the op /
//! decision / limit enums, the op-specific params variants, and NIP-01
//! canonical serialization. The GRANT *is* a `charter_primitives::NostrEvent`
//! (kind = `CHARTER_DEVICE_GRANT`, content = `GrantPayload` JSON, sig = guardian
//! schnorr). This crate has **no crypto / async / IO** dependency.

pub mod apps;
pub mod canonical;
pub mod clause;
pub mod error;
pub mod flatpak_ref;
pub mod grant;
pub mod learning;
pub mod op;
pub mod params;
pub mod release;
pub mod request;
pub mod status;
pub mod usage_sync;

pub use apps::{
    AppHold, AppPosture, GrantApps, HoldState, APPS_VERSION, MAX_APP_HOLDS, MAX_APP_HOLD_SECS,
};
pub use canonical::canonical_event_string;
pub use clause::{
    AlwaysAvailableApp, AlwaysAvailableBody, BreakGlassCfg, BreakGlassScope, ClauseKind,
    ClausePayload, GiftBody, LifelineBody, LifelineNumber, ListeningBody, ListeningMode,
    ListeningVerdict, MaintenanceBody, MaintenanceSpan, StandDownBody, UpdateAppBody,
    CLAUSE_VERSION, DEFAULT_STANDDOWN_GRACE_SECS, MAINTENANCE_SPAN_STORE_KEY,
    MAX_BREAK_GLASS_MINUTES, MAX_GIFT_MINUTES, MAX_LIFELINE_NUMBERS, MAX_LISTENING_GRACE_MINUTES,
    MAX_MAINTENANCE_SECS, MAX_STANDDOWN_GRACE_SECS, STANDDOWN_GRACE_STORE_KEY,
    USAGE_SYNC_STORE_KEY,
};
pub use error::ProtoError;
pub use flatpak_ref::{FlatpakRef, FlatpakRefError};
pub use grant::{GrantPayload, GRANT_VERSION};
pub use learning::{GrantLearning, LearningApp, LearningAppKind, LEARNING_VERSION};
pub use op::{Decision, LimitHit, OpType};
pub use params::{
    AppOpenGrantParams, AppOpenRequestParams, ExecAllowParams, GrantParams, InstallFlatpakParams,
    Remote, TimeExtendGrantParams, TimeExtendRequestParams, MAX_EXTEND_MINUTES, MAX_REASON_LEN,
};
pub use release::ReleasePayload;
pub use request::{RequestPayload, REQUEST_VERSION};
pub use status::{
    AppRef, EnforcementGap, GroupSpent, InstallChange, InstallChangeKind, InstallWindowReport,
    StatusLockReason, StatusPayload, StatusSource, MAX_STATUS_APPS, MAX_STATUS_GROUPS,
    STATUS_VERSION,
};
pub use usage_sync::{UsageSyncPayload, USAGE_SYNC_VERSION};
