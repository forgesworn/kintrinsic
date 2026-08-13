//! The tray must outlive a desktop that isn't ready yet.
//!
//! On Linux Mint the StatusNotifierItem watcher (`xapp-sn-watcher`) is itself an
//! `/etc/xdg/autostart` entry, and `org.kde.StatusNotifierWatcher` is NOT a
//! D-Bus activatable name (only `org.x.StatusNotifierWatcher` is). So at login
//! the tray and the watcher start in the same batch, and the tray usually wins
//! the race: it asks to register before anything owns the watcher name.
//!
//! If that first registration is treated as fatal the tray dies on the spot and
//! never comes back — no icon for the whole session, and logging out doesn't
//! help because the race goes the same way every time. That was the 0.6.0 bug:
//! nothing in the panel on a stock Mint install.
//!
//! This test recreates exactly that: a private session bus with no watcher on
//! it. The tray must stay up and wait for the watcher to appear.
//!
//!   cargo test -p charter-tray --no-default-features --features real \
//!     --test survives_late_watcher
//!
//! The mock build has no bus and no tray, so this is a `real`-only target.

#![cfg(feature = "real")]

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long to give the tray to fall over. The panic was immediate (~100ms);
/// this is generous headroom, not a guess at startup cost.
const GRACE: Duration = Duration::from_secs(3);

#[test]
fn stays_up_on_a_session_bus_with_no_watcher() {
    // `dbus-run-session` is what gives us a bus with nothing on it. Without it
    // there is no way to stage the race, so say so rather than pass silently.
    if Command::new("dbus-run-session")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("SKIPPED: dbus-run-session is not installed; cannot stage an empty session bus");
        return;
    }

    let mut child = Command::new("dbus-run-session")
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_charter-tray"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn charter-tray under a private session bus");

    // Poll rather than sleep the whole grace period: a death is what we're
    // looking for, and it arrives fast when it arrives at all.
    let deadline = Instant::now() + GRACE;
    let mut exited = None;
    while Instant::now() < deadline {
        match child.try_wait().expect("poll charter-tray") {
            Some(status) => {
                exited = Some(status);
                break;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    if let Some(status) = exited {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        panic!(
            "charter-tray quit ({status}) on a bus with no StatusNotifierWatcher — \
             at login that means no tray for the whole session.\nIts stderr was:\n{stderr}"
        );
    }

    let _ = child.kill();
    let _ = child.wait();
}
