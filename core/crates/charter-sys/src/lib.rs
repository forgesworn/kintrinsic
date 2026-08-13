//! `charter-sys` — **THE** system-abstraction layer. Every OS effect Charter
//! for Linux performs sits behind a named, typed port here. Two impls per port:
//! a deterministic in-memory `mock` (feature `mock`, all headless tests) and a
//! compile-only `real` (feature `real`) exercised only on the VM.
//!
//! `charterd` is generic over [`SystemLayer`] so the whole daemon builds,
//! unit-tests, and integration-tests with zero privilege.
//!
//! **Provisional-trait rule:** the IO effect traits in [`effects`] are minimal
//! and explicitly provisional — the owning phase finalizes each signature in a
//! single edit. The persistence ports, [`clock`], and the signers are stable.
//!
//! **No on-device DOB:** no source in this workspace may set/populate/read-for-
//! use systemd's world-readable `birthDate`. The canonical semantic guard lives
//! in `tests/privacy_birthdate_guard.rs` and is the only such guard in the tree.

pub mod clock;
pub mod effects;
pub mod error;
#[cfg(any(feature = "real-relay", feature = "real-os"))]
pub(crate) mod fsutil;
pub mod layer;
pub mod persistence;
pub mod relay;
pub mod signer;

pub use clock::Clock;
pub use error::{SysError, SysResult};
pub use layer::{run, SystemLayer};

#[cfg(feature = "mock")]
pub use layer::MockSystem;

#[cfg(all(feature = "real-relay", feature = "real-os"))]
pub use layer::RealSystem;
