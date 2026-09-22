//! Liveness: telling systemd the enforcement tick is still running, and
//! recording — durably — when it last did.
//!
//! # Why both halves
//!
//! `Restart=on-failure` only covers a daemon that *exits*. A charterd that
//! HANGS — a deadlocked executor, a wedged logind call, which the unit's own
//! comments describe as observed live — is a live process as far as systemd is
//! concerned, so nothing restarts it and everything quietly stops being
//! enforced. `WatchdogSec=` fixes that, but only if somebody sends the
//! keep-alive: [`ping`] is that somebody, called from the enforcement tick
//! itself so the thing being vouched for is the thing that actually runs.
//!
//! The restart is deliberately paired with `ExecStopPost=charter-recovery
//! --thaw-all`, which stays: a wedged daemon must never leave a frozen box.
//! That means every watchdog kill opens a short unenforced window, which is
//! the second half's job —
//!
//! # The stamp
//!
//! Nothing recorded that enforcement had been off. A ward with a live USB, a
//! GRUB `init=/bin/bash`, or a daemon that crash-looped for an afternoon left
//! the guardian looking at a quiet gap in the meter that reads exactly like a
//! quiet evening. The device cannot witness its own absence, but it can write
//! down when it was last awake: [`record`] each tick, [`gap_secs`] on startup.
//! Cheap, and it turns an undetectable bypass into a visible one.

/// Seconds between "the daemon was simply restarted" and "enforcement was off
/// for long enough that somebody should be told". A restart takes seconds; an
/// upgrade takes tens of seconds. Five minutes is well clear of both and still
/// catches a single boot into an unwarded system.
pub const GAP_THRESHOLD_SECS: i64 = 300;

/// Where the last-enforced stamp lives. `/var/lib` (the unit's
/// `StateDirectory=charter`), never `/run`: the whole point is to survive the
/// reboot, which is exactly what `/run` does not do.
pub fn stamp_path() -> String {
    std::env::var("CHARTER_ENFORCED_STAMP")
        .unwrap_or_else(|_| "/var/lib/charter/last-enforced".into())
}

/// Record that the enforcement tick ran at `now` (unix seconds).
///
/// Atomic (temp → fsync → rename, the same writer the usage ledger uses), for
/// the same reason the ledger is: the ward is allowed to hold the power
/// button, so a torn write here is something they can repeat until it lands —
/// and a half-written stamp that parses as a small number would manufacture a
/// gap that never happened.
///
/// Best-effort: a stamp that cannot be written is never a reason to stop
/// enforcing.
pub fn record(now: i64) {
    let _ = crate::atomic_file::atomic_write(&stamp_path(), now.to_string().as_bytes(), 0o600);
}

/// The last recorded enforcement time, or `None` if there is no readable
/// stamp (a fresh install, a purged state dir).
pub fn last_enforced() -> Option<i64> {
    std::fs::read_to_string(stamp_path())
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
}

/// How long enforcement was off before `now`, when that is longer than
/// [`GAP_THRESHOLD_SECS`].
///
/// `None` covers three different innocent things and says so by saying
/// nothing: no stamp at all (first run), an ordinary restart, and a stamp in
/// the FUTURE. The last one is a clock that moved, not a gap — reporting a
/// negative "enforcement was off" figure would be a confident wrong number,
/// and this surface exists precisely because a confident wrong number is worse
/// than silence here.
pub fn gap_secs(now: i64) -> Option<i64> {
    let last = last_enforced()?;
    let gap = now.checked_sub(last)?;
    (gap > GAP_THRESHOLD_SECS).then_some(gap)
}

/// Send `state` to systemd's notify socket (`sd_notify`), returning whether it
/// went out. `false` whenever there is no `$NOTIFY_SOCKET` — i.e. whenever
/// charterd is run by hand — which is not an error.
///
/// Fifteen lines of `sendto` rather than a dependency: the protocol is one
/// unsolicited datagram of `KEY=value` lines to a unix socket, and systemd
/// spells the abstract namespace `@` where the kernel wants a leading NUL.
#[cfg(feature = "real")]
pub fn notify(state: &str) -> bool {
    let Ok(sock) = std::env::var("NOTIFY_SOCKET") else {
        return false;
    };
    if sock.is_empty() {
        return false;
    }
    // SAFETY: a zeroed `sockaddr_un` filled in below, a bounds-checked copy
    // into `sun_path`, and an fd closed on every path out.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return false;
        }
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = sock.as_bytes();
        if bytes.is_empty() || bytes.len() >= addr.sun_path.len() {
            libc::close(fd);
            return false;
        }
        for (i, b) in bytes.iter().enumerate() {
            // systemd writes an abstract socket as "@/org/…"; on the wire the
            // first byte of an abstract address is NUL.
            addr.sun_path[i] = if i == 0 && *b == b'@' {
                0
            } else {
                *b as libc::c_char
            };
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len()) as libc::socklen_t;
        let sent = libc::sendto(
            fd,
            state.as_ptr() as *const libc::c_void,
            state.len(),
            libc::MSG_NOSIGNAL,
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            len,
        );
        libc::close(fd);
        sent >= 0
    }
}

/// The watchdog keep-alive, sent from the enforcement tick.
#[cfg(feature = "real")]
pub fn ping() {
    let _ = notify("WATCHDOG=1");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let d =
            std::env::temp_dir().join(format!("charterd-watchdog-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("last-enforced").to_string_lossy().into_owned()
    }

    /// These exercise the pure stamp arithmetic against a real file, without
    /// the process-global env var (which several tests would race over).
    fn write_stamp(path: &str, at: i64) {
        std::fs::write(path, at.to_string()).unwrap();
    }
    fn read_gap(path: &str, now: i64) -> Option<i64> {
        let last = std::fs::read_to_string(path)
            .ok()?
            .trim()
            .parse::<i64>()
            .ok()?;
        let gap = now.checked_sub(last)?;
        (gap > GAP_THRESHOLD_SECS).then_some(gap)
    }

    #[test]
    fn an_ordinary_restart_is_not_a_gap() {
        let p = fixture("restart");
        write_stamp(&p, 1_000_000);
        assert_eq!(read_gap(&p, 1_000_000 + 20), None);
        assert_eq!(
            read_gap(&p, 1_000_000 + GAP_THRESHOLD_SECS),
            None,
            "the threshold itself is still an ordinary restart"
        );
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p).parent().unwrap());
    }

    #[test]
    fn an_afternoon_off_is_reported() {
        let p = fixture("afternoon");
        write_stamp(&p, 1_000_000);
        assert_eq!(read_gap(&p, 1_000_000 + 4 * 3600), Some(4 * 3600));
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p).parent().unwrap());
    }

    #[test]
    fn a_stamp_from_the_future_is_a_clock_move_not_a_gap() {
        let p = fixture("future");
        write_stamp(&p, 2_000_000);
        assert_eq!(read_gap(&p, 1_000_000), None);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p).parent().unwrap());
    }

    #[test]
    fn a_missing_or_garbage_stamp_reports_nothing() {
        let p = fixture("garbage");
        assert_eq!(read_gap(&p, 1_000_000), None, "no stamp at all");
        std::fs::write(&p, "not a number").unwrap();
        assert_eq!(read_gap(&p, 1_000_000), None);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p).parent().unwrap());
    }

    /// The real `record`/`gap_secs` pair over the env-var seam, kept to ONE
    /// test so nothing races on the variable.
    #[test]
    fn record_then_gap_round_trips_through_the_real_path() {
        let p = fixture("roundtrip");
        std::env::set_var("CHARTER_ENFORCED_STAMP", &p);
        record(1_500_000);
        assert_eq!(last_enforced(), Some(1_500_000));
        assert_eq!(gap_secs(1_500_030), None);
        assert_eq!(gap_secs(1_500_000 + 900), Some(900));
        std::env::remove_var("CHARTER_ENFORCED_STAMP");
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p).parent().unwrap());
    }

    /// No `$NOTIFY_SOCKET` (charterd run by hand) is not an error.
    #[cfg(feature = "real")]
    #[test]
    fn notify_without_a_socket_is_a_quiet_false() {
        std::env::remove_var("NOTIFY_SOCKET");
        assert!(!notify("WATCHDOG=1"));
    }
}
