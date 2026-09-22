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
    /// An administrator has PAUSED enforcement on this machine (the Recovery
    /// tool). Nothing is being enforced against this child right now, and the
    /// numbers above are frozen where the pause found them.
    ///
    /// Published rather than implied by a stale `at`: a paused stretch used to
    /// look exactly like a daemon that had stopped ticking, and the two want
    /// opposite reactions from whoever is reading. (03-G5)
    #[serde(default)]
    pub paused_by_admin: bool,
    /// Seconds this machine went WITHOUT a running warden before the daemon
    /// came back — `None` when there was no such gap, never `0`.
    ///
    /// The box cannot witness its own absence, but it can write down when it
    /// was last awake and notice the hole on the way back up: a live USB, a
    /// GRUB `init=/bin/bash`, a crash-loop, an afternoon powered off with the
    /// disk in another machine. A FACT, not an accusation — a long holiday
    /// produces one too. (04-G6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_gap_secs: Option<i64>,
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

/// Keep publishing while an admin pause is in force (03-G5).
///
/// The loop thaws everything and goes idle during a pause, which used to mean
/// it skipped the state-file publish entirely: the ward's console and the
/// guardian's view simply stopped updating, showing the last pre-pause numbers
/// as though they were current. Silence is the one failure neither of them can
/// see. So the last published snapshot is re-stamped with a fresh `at` and an
/// explicit `paused_by_admin`, which says the thing the frozen numbers cannot:
/// nothing is being enforced right now, ON PURPOSE.
///
/// Only ever re-stamps a file that already exists — there are no live numbers
/// to invent for a child who has none, and a pause is not the moment to start
/// guessing.
pub fn mark_paused(user: &str, at: i64) {
    let path = format!("{}/{user}.json", state_dir());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut state) = serde_json::from_str::<PublishedState>(&text) else {
        return;
    };
    if state.paused_by_admin && state.at == at {
        return;
    }
    state.at = at;
    state.paused_by_admin = true;
    publish(&state);
}

/// Every child this machine is currently publishing state for.
pub fn published_users() -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(state_dir()) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            Some(name.strip_suffix(".json")?.to_string())
        })
        .collect()
}

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
            paused_by_admin: false,
            enforcement_gap_secs: None,
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

        // 03-G5: a pause keeps the file MOVING, and says why the numbers are
        // frozen — a state file that simply stops updating is indistinguishable
        // from a daemon that has died, and the two want opposite reactions.
        publish(&state("robin"));
        mark_paused("robin", 1_782_738_000);
        let paused: PublishedState =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(paused.paused_by_admin, "the pause is stated, not implied");
        assert_eq!(paused.at, 1_782_738_000, "and the snapshot is fresh");
        assert_eq!(
            paused.time_left.used_today_seconds,
            Some(5400),
            "the numbers are carried through untouched, frozen where the pause found them"
        );
        assert_eq!(published_users(), vec!["robin".to_string()]);

        // A child with no published state is not invented during a pause.
        mark_paused("nobody", 1_782_738_000);
        assert!(!dir.join("nobody.json").exists());

        retract("robin");
        assert!(!path.exists());
        assert!(published_users().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        std::env::remove_var("CHARTER_STATE_DIR");
    }

    /// An older state file (written before these fields existed) must still
    /// load — a console reading it mid-upgrade sees "not paused, no gap",
    /// which is the truthful default, not a parse failure.
    #[test]
    fn a_pre_existing_state_file_without_the_new_fields_still_parses() {
        let s = state("robin");
        let mut v = serde_json::to_value(&s).unwrap();
        let o = v.as_object_mut().unwrap();
        o.remove("pausedByAdmin");
        o.remove("enforcementGapSecs");
        let back: PublishedState = serde_json::from_value(v).unwrap();
        assert!(!back.paused_by_admin);
        assert_eq!(back.enforcement_gap_secs, None);
    }

    /// The gap is absent-or-a-number, never a zero: "0 seconds unwarded" is a
    /// line no surface should ever grow, and coercing absent to 0 would claim
    /// a clean bill of health from a device that has simply never reported.
    #[test]
    fn the_enforcement_gap_is_omitted_when_there_is_none() {
        let mut s = state("robin");
        assert!(!serde_json::to_string(&s)
            .unwrap()
            .contains("enforcementGap"));
        s.enforcement_gap_secs = Some(4 * 3600);
        let wire = serde_json::to_string(&s).unwrap();
        assert!(wire.contains("\"enforcementGapSecs\":14400"), "{wire}");
    }
}
