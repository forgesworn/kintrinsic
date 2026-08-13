//! The world-readable live state each managed child's console reads.
//!
//! `TimeLeft` over D-Bus answers for the **calling** uid, which is exactly
//! right for the ward's own tray and CLI and useless for the guardian: an
//! admin asking about their child gets their own (unmanaged, unbounded)
//! answer. The obvious fix — a `TimeLeftFor(uid)` method — would widen a
//! deliberately frozen five-method contract for a read the daemon can simply
//! publish.
//!
//! So the loop writes each child's live state to `/run/charter/state/<user>.json`
//! every tick. `/run` because this is a cache of something the daemon already
//! knows: it must never outlive the daemon that produced it, and a stale
//! snapshot left behind by a crash would be a screen full of confident wrong
//! numbers. `RuntimeDirectory=charter` clears it on every restart for free.
//!
//! World-readable is a decision, not an oversight. Kintrinsic's standing rule is
//! that a ward can always see what is being enforced against them; a device
//! that hides its own limits is the thing this product exists to replace. The
//! file carries no secrets — no keys, no guardian identity, no browsing or app
//! history — only the numbers already shown on the ward's own lock screen.

use charter_ipc::dto::TimeLeftView;
use serde::{Deserialize, Serialize};

/// The directory the per-child state files live in.
pub fn state_dir() -> String {
    std::env::var("CHARTER_STATE_DIR").unwrap_or_else(|_| "/run/charter/state".into())
}

/// One managed child's published state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedState {
    pub v: u32,
    /// The account this describes.
    pub user: String,
    /// When the loop wrote it — the console shows the numbers as stale rather
    /// than confidently wrong if the daemon has stopped ticking.
    pub at: i64,
    /// The live breakdown, buckets included.
    pub time_left: TimeLeftView,
    /// Net minutes given (or taken back) at this computer today.
    #[serde(default)]
    pub local_adjust_minutes: i32,
    /// Whether a guardian's phone is bound to this child. `false` is the
    /// standalone case — the console is the only management surface there is.
    pub paired: bool,
}

pub const STATE_VERSION: u32 = 1;

/// Write one child's state. Best-effort: a failure here must never disturb
/// enforcement, which is the thing that actually matters. Written via a
/// temp-file rename so a console reading concurrently sees either the old
/// snapshot or the new one, never a half-written one.
pub fn publish(state: &PublishedState) {
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(body) = serde_json::to_string(state) else {
        return;
    };
    let final_path = format!("{dir}/{}.json", state.user);
    let tmp_path = format!("{final_path}.tmp");
    if std::fs::write(&tmp_path, &body).is_err() {
        return;
    }
    set_world_readable(&tmp_path);
    if std::fs::rename(&tmp_path, &final_path).is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// 0644 — every local account may read, only root may write. The daemon runs
/// with root's umask, which would otherwise leave this 0600 and blind the very
/// consoles it exists to feed.
#[cfg(unix)]
fn set_world_readable(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
}

#[cfg(not(unix))]
fn set_world_readable(_path: &str) {}

/// Remove the state file for a child who is no longer managed, so a console
/// can never render limits for an account that has been released.
pub fn retract(user: &str) {
    let _ = std::fs::remove_file(format!("{}/{user}.json", state_dir()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> TimeLeftView {
        TimeLeftView {
            effective_seconds: 1800,
            schedule_seconds: 7200,
            budget_seconds: 1800,
            extension_seconds: 0,
            locked: false,
            reason: None,
            next_open: None,
            offline: false,
            learning_today_seconds: None,
            used_today_seconds: Some(5400),
            buckets: Vec::new(),
            budget_day_seconds: -1,
            budget_week_seconds: -1,
            ask_first: Vec::new(),
        }
    }

    fn state(user: &str) -> PublishedState {
        PublishedState {
            v: STATE_VERSION,
            user: user.into(),
            at: 1_782_734_400,
            time_left: view(),
            local_adjust_minutes: -20,
            paired: false,
        }
    }

    #[test]
    fn a_published_state_roundtrips_through_camel_case_json() {
        let s = state("robin");
        let wire = serde_json::to_string(&s).unwrap();
        assert!(wire.contains("\"localAdjustMinutes\":-20"), "{wire}");
        assert!(wire.contains("\"timeLeft\""), "{wire}");
        let back: PublishedState = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, s);
    }

    /// The `askFirst` list (with labels) rides along inside `timeLeft`, so a
    /// console reading the world-readable file can build "Ask to open" rows
    /// with no policy read of its own.
    #[test]
    fn ask_first_apps_ride_along_in_the_published_state() {
        use charter_ipc::dto::AskFirstAppView;
        let mut s = state("robin");
        s.time_left.ask_first = vec![AskFirstAppView {
            pkg: "com.mojang.minecraftpe".into(),
            label: "Minecraft".into(),
        }];
        let wire = serde_json::to_string(&s).unwrap();
        assert!(wire.contains("\"ask_first\""), "{wire}");
        assert!(wire.contains("Minecraft"), "{wire}");
        let back: PublishedState = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.time_left.ask_first, s.time_left.ask_first);
    }

    #[test]
    fn publish_writes_a_world_readable_file_and_retract_removes_it() {
        let dir = std::env::temp_dir().join(format!("charter-state-test-{}", std::process::id()));
        std::env::set_var("CHARTER_STATE_DIR", &dir);

        publish(&state("robin"));
        let path = dir.join("robin.json");
        let text = std::fs::read_to_string(&path).expect("state file written");
        let back: PublishedState = serde_json::from_str(&text).unwrap();
        assert_eq!(back.user, "robin");
        assert_eq!(back.time_left.used_today_seconds, Some(5400));

        // Readable by the ward, writable only by the daemon's own user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o044, 0o044, "world+group readable");
            assert_eq!(mode & 0o022, 0, "not writable by anyone but the owner");
        }

        // No temp file is left behind for a console to trip over.
        assert!(!dir.join("robin.json.tmp").exists());

        retract("robin");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
        std::env::remove_var("CHARTER_STATE_DIR");
    }
}
