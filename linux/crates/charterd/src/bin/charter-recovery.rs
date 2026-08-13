//! `charter-recovery` — the admin escape hatch so a parent can never brick the
//! family machine. Pause/resume/disable enforcement, and `--thaw-all` (used by
//! the unit's ExecStopPost + the Disable action) to unfreeze everything.
//!
//! **Security (admin-only, three independent layers):** (1) the menu launcher is
//! pkexec'd and `49-charter.rules` denies `org.freedesktop.policykit.exec` to the
//! `charter-managed` group; (2) pkexec needs an admin password the child doesn't
//! have; (3) **authoritative:** this binary refuses unless its effective uid is 0
//! — and the managed child is structurally never root (sudo stripped). So even a
//! direct `charter-recovery --thaw-all` by the child is a guaranteed no-op,
//! independent of the polkit file being correct. A managed-child pkexec caller is
//! additionally refused by name (belt-and-suspenders audit).
//!
//! Real-gated (uses the live cgroup/VT ports); the mock build is a stub so the
//! headless gate stays green.

#[cfg(feature = "real")]
fn main() {
    use std::process::Command;

    // (1) AUTHORITATIVE identity gate: root only. The child is never root.
    if effective_uid() != Some(0) {
        eprintln!("charter-recovery: must run as the administrator (use the menu).");
        std::process::exit(1);
    }

    let thaw_only = std::env::args().any(|a| a == "--thaw-all");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("charter-recovery: tokio runtime");

    if thaw_only {
        rt.block_on(thaw_everything());
        return;
    }

    // (3) belt-and-suspenders: refuse a managed-child pkexec caller by name.
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let pkexec_uid = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|s| s.parse().ok());
    if charterd::managed_guard::caller_is_managed_child(&passwd, &group, pkexec_uid) {
        let _ = zenity(&[
            "--error",
            "--title=Kintrinsic — Recovery",
            "--text=Recovery is for the computer's administrator only.",
        ]);
        eprintln!("charter-recovery: refused — caller is a managed child (uid {pkexec_uid:?})");
        std::process::exit(1);
    }

    // Menu.
    let choice = match zenity(&[
        "--list",
        "--title=Kintrinsic — Recovery",
        "--text=Kintrinsic is enforcing limits on this computer. What would you like to do?",
        "--column=Action",
        "--hide-header",
        "Pause — unfreeze now, keep the settings",
        "Resume — start enforcing limits again",
        "Turn Kintrinsic off completely",
    ]) {
        Some(out) if !out.is_empty() => out,
        _ => return, // cancelled
    };

    if choice.starts_with("Pause") {
        if let Some(dir) = std::path::Path::new(&pause_flag()).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(pause_flag(), b"paused\n") {
            Ok(()) => {
                rt.block_on(thaw_everything());
                notify("--info", "Kintrinsic is paused — the computer is usable now. Open Recovery again to resume.");
            }
            Err(e) => notify("--error", &format!("Could not pause: {e}")),
        }
    } else if choice.starts_with("Resume") {
        let _ = std::fs::remove_file(pause_flag());
        notify("--info", "Kintrinsic is enforcing limits again.");
    } else if choice.starts_with("Turn Kintrinsic off") {
        let confirm = zenity(&[
            "--question",
            "--title=Kintrinsic — Recovery",
            "--text=Turn Kintrinsic off completely? Limits stop until you re-enable it from Kintrinsic Setup.",
        ]);
        if confirm.is_some() {
            let _ = Command::new("systemctl")
                .args(["disable", "--now", "charterd"])
                .status();
            rt.block_on(thaw_everything());
            notify(
                "--info",
                "Kintrinsic is turned off. Re-enable it from Kintrinsic Setup.",
            );
        }
    }
}

/// The recovery pause flag (matches `runtime::pause_flag_path`).
#[cfg(feature = "real")]
fn pause_flag() -> String {
    std::env::var("CHARTER_PAUSE_FLAG").unwrap_or_else(|_| "/run/charter/paused".into())
}

/// Effective uid from `/proc/self/status` (`Uid: real eff saved fs`). `None` =
/// couldn't determine → the caller treats it as non-root (fail-closed).
#[cfg(feature = "real")]
fn effective_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

/// Thaw every managed child's app.slice, re-enable VT switching, kill the lock —
/// the box becomes usable. Reuses the same ports + roster the daemon enforces with.
#[cfg(feature = "real")]
async fn thaw_everything() {
    use charter_schedule::is_valid_freeze_target;
    use charter_sys::effects::{CgroupFreezer, RealCgroupFreezer, RealVtControl, VtControl};
    use charterd::enforcer_runtime::managed_freeze_target;
    use charterd::managed_guard::ManagedRoster;

    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let roster = ManagedRoster::from_system(&passwd, &group);
    let freezer = RealCgroupFreezer::default();
    for uid in roster.uids() {
        let slice = managed_freeze_target(uid);
        if is_valid_freeze_target(&slice) {
            let _ = freezer.thaw(&slice).await;
        }
    }
    let _ = RealVtControl.set_vt_switching(true).await;
    let _ = std::process::Command::new("pkill")
        .args(["-x", "charter-lock"])
        .status();
}

#[cfg(feature = "real")]
fn zenity(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("zenity")
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(feature = "real")]
fn notify(flag: &str, text: &str) {
    let _ = zenity(&[
        flag,
        "--title=Kintrinsic — Recovery",
        &format!("--text={text}"),
    ]);
}

#[cfg(not(feature = "real"))]
fn main() {
    eprintln!("charter-recovery: the recovery effects (thaw/VT/disable) bind on a provisioned host built with --features real; this is the mock skeleton.");
}
