//! `charter-spine` — the OS-agnostic broker spine shared by every warden.
//!
//! A fail-closed lifecycle reducer, a single-use-atomic broker, the enactor
//! registry, the multi-child enforcer, and the status/lock-info projections —
//! extracted from `charterd` (port-spec §1.3) so the Linux daemon and the
//! Android Device Owner enforcer run the SAME security spine. Generic over
//! `SystemLayer` + a `TransportFacade`, so the whole spine builds and tests
//! with zero privilege against mocks. The OS-effect halves (cgroup freeze, VT
//! lock, DevicePolicyManager suspension) live with their platforms; nothing in
//! this crate performs an OS call.

pub mod audit;
pub mod broker;
pub mod child_policy;
pub mod curator_sync;
pub mod device_code;
pub mod enactor;
pub mod enactors;
pub mod enforce_mode;
pub mod enforcer_runtime;
pub mod error;
pub mod lifecycle;
pub mod local_limits;
pub mod lock_info;
pub mod multi_child;
pub mod ports;
pub mod standdown;
pub mod status_emit;
pub mod transport_facade;
pub mod usage_pool;

pub use audit::{AuditOutcome, AuditRecord};
pub use broker::Broker;
pub use enactor::{EnactOutcome, Enactor, EnactorRegistry};
pub use error::{BrokerError, EnactError};
pub use lifecycle::{transition, Effect, Event, RequestRecord, RequestState};
pub use ports::{EventSink, TimeLeftProvider, TimeLeftSnapshot, TimeLeftState};
pub use transport_facade::{MockTransport, TransportFacade};
