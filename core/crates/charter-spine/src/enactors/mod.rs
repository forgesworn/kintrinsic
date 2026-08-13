//! Enactors that are part of the shared spine. `time.extend` runs identically
//! on every warden (pure ledger math + the signed-grant contract); OS-coupled
//! enactors (Flatpak install, exec-allow, APK install) live with their
//! platforms and register into the same [`crate::EnactorRegistry`].
//! `app.open` is also shared (and trivial): it enacts nothing at all — see
//! [`app_open`]'s module doc for why it still needs registering.

pub mod app_open;
pub mod time_extend;

pub use app_open::AppOpenEnactor;
pub use time_extend::TimeExtendEnactor;
