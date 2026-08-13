//! The Linux enforce half over the shared spine's [`EnforcerRuntime`]: uid →
//! cgroup-slice resolution and the async Freeze/Thaw + ShowLock/HideLock + VT
//! port applier. The decision core (tick/ledgers/EOD/lock copy) lives in
//! `charter_spine::enforcer_runtime` — shared verbatim with the Android
//! warden; everything here is what only a Linux box can do.

pub use charter_spine::enforcer_runtime::*;

use charter_schedule::{is_valid_freeze_target, EnforcerEffect};
use charter_sys::effects::{CgroupFreezer, SessionControl, VtControl};
use charter_sys::SystemLayer;

/// The managed child's freeze target — their **whole user slice**,
/// `user.slice/user-<uid>.slice` (relative to the cgroup-v2 unified root). This
/// covers BOTH the logind desktop session scope (`session-<id>.scope`, where
/// Cinnamon/Mint runs the panel + GUI apps) AND the systemd-user `app.slice`.
/// The earlier `app.slice`-only target froze neither the desktop nor the apps on
/// Cinnamon (they live in the session scope) — only a few background services
/// paused. The X server runs under `lightdm.service` in `system.slice`, NOT the
/// user slice, so freezing the whole user slice still lets the (root) lock screen
/// draw. Derived from the managed uid the daemon resolves at boot; the abstract
/// target the pure core emits is still `FreezeTarget::ManagedAppSlice`. This is
/// the only place uid → cgroup path is resolved, and it is always run through
/// `is_valid_freeze_target` (never root/system/charterd/lock).
pub fn managed_freeze_target(uid: u32) -> String {
    format!("user.slice/user-{uid}.slice")
}

/// Apply the async port effects of a tick (freeze/thaw + lock + VT). Holds no
/// lock — call it *after* releasing any `Mutex<EnforcerRuntime>` guard. The
/// concrete `app_slice` is resolved by the caller from the managed uid
/// ([`managed_freeze_target`]).
pub async fn apply_effects<S: SystemLayer>(sys: &S, effects: &[EnforcerEffect], app_slice: &str) {
    for e in effects {
        match e {
            EnforcerEffect::Freeze(_) => {
                // Structural guard: never freeze anything but a managed user slice.
                if is_valid_freeze_target(app_slice) {
                    if let Err(e) = sys.freezer().freeze(app_slice).await {
                        eprintln!("charterd: freeze {app_slice} failed: {e}");
                    }
                }
            }
            EnforcerEffect::Thaw(_) => {
                if let Err(e) = sys.freezer().thaw(app_slice).await {
                    eprintln!("charterd: thaw {app_slice} failed: {e}");
                }
            }
            EnforcerEffect::ShowLock(reason) => {
                let (title, detail) = lock_message(*reason);
                let _ = sys.session().show_lock(title, detail).await;
                let _ = sys.vt().set_vt_switching(false).await;
            }
            EnforcerEffect::HideLock => {
                let _ = sys.session().hide_lock().await;
                let _ = sys.vt().set_vt_switching(true).await;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_freeze_target_is_the_whole_user_slice() {
        // The child's whole user slice — covers the logind desktop session scope
        // AND app.slice, so the freeze actually reaches the apps on Cinnamon.
        assert_eq!(managed_freeze_target(1002), "user.slice/user-1002.slice");
        // Accepted by the freeze-target guard (a user slice, not root/system/
        // charterd/lock).
        assert!(charter_schedule::is_valid_freeze_target(
            &managed_freeze_target(1002)
        ));
    }
}
