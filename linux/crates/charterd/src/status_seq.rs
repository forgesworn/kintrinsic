//! Durable storage for the STATUS `seq` counter (R2-2/R2-3). The arithmetic
//! lives in `status_emit::StatusSeq` (shared with Android); this is only
//! where charterd keeps the last value it handed out.
//!
//! `/var/lib` (the unit's `StateDirectory=charter`), never `/run`: the whole
//! point is that the sequence survives a reboot, and a ward can force one.

use crate::status_emit::StatusSeq;

/// Where the last emitted `seq` lives.
pub fn path() -> String {
    std::env::var("CHARTER_STATUS_SEQ").unwrap_or_else(|_| "/var/lib/charter/status-seq".into())
}

/// The stored value at `path`, or `None` when it is missing or unreadable
/// (which [`StatusSeq::resume`] answers from the wall clock).
pub fn load_from(path: &str) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| StatusSeq::parse_stored(&s))
}

/// Store `seq` at `path` atomically (temp → fsync → rename), so a torn write
/// can never leave a smaller number behind: the file holds either the old
/// value or the new one.
pub fn store_to(path: &str, seq: u64) -> std::io::Result<()> {
    crate::atomic_file::atomic_write(path, StatusSeq::format_stored(seq).as_bytes(), 0o600)
}

/// Resume the counter from [`path`] and the wall clock now.
pub fn resume(now_unix_ms: u64) -> StatusSeq {
    StatusSeq::resume(load_from(&path()), now_unix_ms)
}

/// Take the next `seq` and store it before the caller publishes it.
///
/// A store that fails is logged and the value is still returned: withholding
/// the STATUS would blind the guardian, which is worse than the risk it
/// guards against. The in-memory counter stays strictly increasing for this
/// run, and a restart re-seeds from the higher of the stored value and the
/// clock in milliseconds, which is far ahead of one emit per heartbeat.
pub fn next_durable(seq: &mut StatusSeq) -> u64 {
    let n = seq.advance();
    // R3-3, accepted residual: publish-anyway can let seq regress after a restart; bounded by R3-H1's fail-safe (usage_save_guard).
    if let Err(e) = store_to(&path(), n) {
        eprintln!("charterd: could not store the STATUS seq ({e}); publishing anyway");
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("charterd-seq-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("status-seq").to_string_lossy().into_owned()
    }

    #[test]
    fn a_stored_seq_survives_a_restart_with_the_clock_set_back() {
        let p = tmp("restart");
        let mut s = StatusSeq::resume(load_from(&p), 1_790_000_000_000);
        let a = s.advance();
        store_to(&p, a).unwrap();
        let b = s.advance();
        store_to(&p, b).unwrap();
        // Restart, clock at the epoch.
        let mut r = StatusSeq::resume(load_from(&p), 0);
        assert_eq!(r.advance(), b + 1);
    }

    #[test]
    fn a_missing_or_unreadable_file_seeds_from_the_clock() {
        let p = tmp("missing");
        assert_eq!(load_from(&p), None);
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).unwrap();
        std::fs::write(&p, b"12\x00garbage").unwrap();
        assert_eq!(load_from(&p), None);
        let mut s = StatusSeq::resume(load_from(&p), 1_790_000_000_000);
        assert_eq!(s.advance(), 1_790_000_000_001);
    }

    #[test]
    fn the_stored_file_is_root_only() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("mode");
        store_to(&p, 7).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load_from(&p), Some(7));
    }
}
