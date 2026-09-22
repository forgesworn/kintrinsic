//! Replacing one of the daemon's own state files, durably.
//!
//! The ward is allowed to reboot this machine and is allowed to hold the power
//! button — the polkit rules grant the first deliberately, and nothing can stop
//! the second. So "a power cut during a write" is not a rare accident here, it
//! is an attack the ward can repeat until it lands: `std::fs::write` opens with
//! `O_TRUNC` and then writes, and a cut inside that window leaves a zero-byte
//! or half-written file. For the usage ledger that reads back as a fresh day
//! with nothing spent.
//!
//! Temp file → `write_all` → `sync_all` → `rename` closes it. A reader sees
//! either the old file or the new one and never a torn one, and the fsync
//! means the bytes are really on the platter before the old name is dropped.
//! The parent directory is fsynced afterwards, best-effort, because the rename
//! itself is a directory change and is otherwise no more durable than the write
//! we just took care over.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Remove `<name>.tmp` and `<name>.<pid>.<n>.tmp` siblings of `target` left by
/// a crash or by the pre-hardening writer. Best-effort: a sweep that fails
/// costs nothing but a stale file.
fn sweep_stale_tmps(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return;
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    let me = std::process::id();
    for e in rd.flatten() {
        let f = e.file_name();
        let f = f.to_string_lossy();
        if !f.starts_with(&prefix) || !f.ends_with(".tmp") {
            continue;
        }
        // `<name>.<pid>.<n>.tmp` from a process still running — ours
        // included — is a write IN FLIGHT, not a leftover: sweeping it would
        // pull the file out from under that writer's rename. Only the bare
        // pre-hardening `<name>.tmp` and a dead process's temps go.
        let owner: Option<u32> = f[prefix.len()..]
            .split('.')
            .next()
            .and_then(|pid| pid.parse().ok());
        let in_flight = match owner {
            Some(pid) if pid == me => true,
            Some(pid) => Path::new(&format!("/proc/{pid}")).exists(),
            None => false,
        };
        if !in_flight {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Write `bytes` to `path` atomically, leaving the file at `mode`.
///
/// The mode is set AT OPEN rather than after the write, and on a file this
/// call created: a write-then-chmod leaves the contents readable under root's
/// 022 umask for the length of the write. `OpenOptions::mode` is itself masked
/// by that umask, so the explicit `set_permissions` on the open handle is what
/// actually guarantees the mode — the same belt-and-braces `pair_token::mint`
/// uses for the pairing token.
///
/// A leftover `.tmp` (from a crash, or from before this was hardened) is
/// removed first, so `create_new` can never inherit an old file's permissions.
pub fn atomic_write(path: &str, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let target = Path::new(path);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Unique per process and per call: two writers to one path (a second
    // daemon task, a helper binary) could otherwise unlink each other's tmp
    // and rename a half-written file into place. Leftovers from a crash are
    // swept up so `create_new` never inherits an old file's mode.
    let tmp = format!(
        "{path}.{}.{}.tmp",
        std::process::id(),
        TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    sweep_stale_tmps(target);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        // Do not leave the half-finished write lying around to confuse the
        // next pass, or to be renamed into place by a later one.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Best-effort: the rename is already visible to every reader on this
    // running kernel, and a failure to fsync the directory costs durability
    // across a power cut, not correctness now. Never fatal to enforcement.
    if let Some(dir) = target.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("charterd-atomic-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.to_string_lossy().into_owned()
    }

    fn mode_of(path: &str) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn it_writes_the_bytes_at_the_mode_asked_for_and_leaves_no_temp_behind() {
        let dir = tmpdir("basic");
        let path = format!("{dir}/nested/one.json");
        atomic_write(&path, b"{\"a\":1}", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        assert_eq!(mode_of(&path), 0o600, "0600, not whatever the umask allows");
        let leftovers = std::fs::read_dir(format!("{dir}/nested"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn it_replaces_a_file_that_was_created_world_readable() {
        // A ledger written before this was hardened is 0644 and a plain write
        // would keep it that way forever, so the replacement has to carry the
        // mode rather than inherit one.
        let dir = tmpdir("regrade");
        std::fs::create_dir_all(&dir).unwrap();
        let path = format!("{dir}/two.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        atomic_write(&path, b"new", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn a_stale_temp_file_does_not_wedge_the_next_write() {
        // `create_new` fails outright on an existing temp, so a crash that
        // left one behind would otherwise stop every later write dead.
        let dir = tmpdir("stale");
        std::fs::create_dir_all(&dir).unwrap();
        let path = format!("{dir}/three.json");
        std::fs::write(format!("{path}.tmp"), "half-written").unwrap();
        std::fs::write(format!("{path}.4194304999.7.tmp"), "half-written").unwrap();
        // A neighbour that merely shares the stem is NOT a leftover.
        std::fs::write(format!("{dir}/three.json.extension.json"), "keep").unwrap();

        atomic_write(&path, b"whole", 0o600).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "whole");
        assert!(!std::path::Path::new(&format!("{path}.tmp")).exists());
        assert!(!std::path::Path::new(&format!("{path}.4194304999.7.tmp")).exists());
        assert!(std::path::Path::new(&format!("{dir}/three.json.extension.json")).exists());
    }

    #[test]
    fn two_writers_to_one_path_never_share_a_temp_name() {
        // With one `<path>.tmp`, writer B could unlink A's temp and A could
        // then rename B's half-written file into place.
        let dir = tmpdir("concurrent");
        let path = format!("{dir}/four.json");
        let handles: Vec<_> = (0..8u8)
            .map(|i| {
                let p = path.clone();
                std::thread::spawn(move || atomic_write(&p, &[b'0' + i; 4096], 0o600))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let got = std::fs::read(&path).unwrap();
        assert_eq!(
            got.len(),
            4096,
            "always one writer's whole file, never a torn one"
        );
        assert!(got.iter().all(|b| *b == got[0]));
    }
}
