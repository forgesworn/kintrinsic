//! `charter-learn <app-id>` — launches one educational site app.
//!
//! The `.desktop` launcher Kintrinsic materialises points here rather than at the
//! browser, for two reasons that both come down to the profile directory:
//!
//! 1. `.desktop` `Exec=` is not run through a shell, so `$HOME` never expands.
//! 2. The profile MUST be per-child. The enactor writes ONE launcher per app
//!    across the union of every managed child, so a fixed system-wide
//!    `--user-data-dir` would have two children on one laptop sharing a Khan
//!    login. Only something running AS the child can resolve their home.
//!
//! And the profile has to exist at all, or an already-open browser session
//! swallows the `--app` launch and the resolver pin — the thing that makes the
//! window safe — is silently discarded.
//!
//! # Not privileged
//!
//! This is a plain binary, NOT setuid: it runs as whoever clicked the menu
//! entry and does nothing they could not do themselves. Its only inputs are
//! root-owned manifests under `/var/lib/charter/learn`, so a ward cannot widen
//! their own pin by editing one — and if they hand-roll the same command line
//! themselves, they get the same sandbox, which is the point (see
//! `charterd::site_app`).

use std::os::unix::process::CommandExt as _;

use charterd::enactors::learning_apps::launch_argv_from_manifest;
use charterd::site_app;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(id) = args.next() else {
        eprintln!("usage: charter-learn <app-id>");
        return std::process::ExitCode::from(2);
    };

    let path = site_app::manifest_path(&id);
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("charter-learn: no launch manifest at {path}: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    let Some(home) = home_dir() else {
        eprintln!("charter-learn: cannot resolve the current user's home directory");
        return std::process::ExitCode::from(1);
    };

    let argv = match launch_argv_from_manifest(&raw, &home) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("charter-learn: {path}: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    // Create the profile up front. Chromium would create it anyway, but a
    // failure here (read-only home, quota) is worth reporting as itself rather
    // than as a confusing browser error.
    let profile = site_app::profile_dir(&home, &id);
    if let Err(e) = std::fs::create_dir_all(&profile) {
        eprintln!("charter-learn: cannot create the profile at {profile}: {e}");
        return std::process::ExitCode::from(1);
    }

    let (runtime, flags) = argv.split_first().expect("sanctioned_argv is never empty");

    // exec rather than spawn: the launcher process becomes the browser, so the
    // window's own /proc/<pid>/cmdline is exactly what site_app renders and the
    // sweep expects. A spawned child would leave this shim as a stray parent
    // and put an extra hop in every ancestry walk.
    //
    // arg0 is set explicitly: Chrome's /usr/bin wrapper re-execs its real
    // binary with `exec -a "$0"`, so argv0 is what survives to identify the
    // process, and it must be the runtime path the checker knows.
    let err = std::process::Command::new(runtime)
        .arg0(runtime)
        .args(flags)
        .exec();

    eprintln!("charter-learn: cannot start {runtime}: {err}");
    std::process::ExitCode::from(1)
}

/// The invoking user's home. `$HOME` is the normal answer; the passwd database
/// is the fallback for a desktop session that somehow launched without it.
fn home_dir() -> Option<String> {
    if let Some(h) = std::env::var_os("HOME") {
        let h = h.to_string_lossy().into_owned();
        if !h.is_empty() {
            return Some(h);
        }
    }
    passwd_home(self_uid()?)
}

/// Our own real uid, read from `/proc/self/status` rather than `getuid(2)` —
/// `libc` is a `real`-feature dependency and this shim builds under both.
/// Format: `Uid:\treal\teffective\tsaved\tfs`.
fn self_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Look up a uid's home directory in `/etc/passwd`.
fn passwd_home(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut f = line.split(':');
        let (_name, _pw, u, _g, _gecos, home) = (
            f.next()?,
            f.next()?,
            f.next()?,
            f.next()?,
            f.next()?,
            f.next()?,
        );
        (u.parse::<u32>().ok()? == uid && !home.is_empty()).then(|| home.to_string())
    })
}
