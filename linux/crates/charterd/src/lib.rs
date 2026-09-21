//! `charterd` — the privileged Linux warden over the shared broker spine.
//! The fail-closed lifecycle reducer, single-use-atomic broker, enactor
//! registry, and multi-child enforcer live in `charter-spine` (shared verbatim
//! with the Android warden) and are re-exported here under their historical
//! paths. What remains local is what only a Linux box can do: the runtime
//! loop, cgroup freeze targets, the confused-deputy exec guard, `/etc/charter`
//! persistence, and the D-Bus surface contract. Generic over `SystemLayer` +
//! a `TransportFacade` so the whole daemon builds and tests with zero
//! privilege against mocks.

// The shared spine, under the same module paths charterd always had.
pub use charter_spine::{
    audit, broker, child_policy, curator_sync, device_code, enactor, enforce_mode, error,
    lifecycle, lock_info, multi_child, ports, status_emit, transport_facade, usage_pool,
};

pub mod ancestry;
pub mod app_inventory;
pub mod app_rules;
pub mod apps_policy;
pub mod atomic_file;
#[cfg(feature = "real")]
pub mod dbus_service;
pub mod dbus_surface;
pub mod device_limits;
pub mod enactors;
pub mod enforcer_runtime;
pub mod exec_guard;
#[cfg(feature = "real")]
pub mod focus;
pub mod local_adjust;
pub mod managed_guard;
pub mod pair_accept;
pub mod pair_commit;
pub mod pair_listener;
pub mod pair_token;
pub mod pairing_setup;
pub mod release_check;
#[cfg(feature = "real")]
pub mod runtime;
pub mod site_app;
pub mod state_file;
pub mod version;
pub mod web_content;

pub use charter_spine::{
    transition, AuditOutcome, AuditRecord, Broker, BrokerError, Effect, EnactError, EnactOutcome,
    Enactor, EnactorRegistry, Event, EventSink, MockTransport, RequestRecord, RequestState,
    TimeLeftProvider, TimeLeftSnapshot, TimeLeftState, TransportFacade,
};
pub use exec_guard::{validate_exec_candidate, ExecPathError, FileKind, ProbeExec};
