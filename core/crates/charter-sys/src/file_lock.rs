//! An exclusive, cross-process advisory lock for a read-modify-write of a
//! record another process also rewrites (review 2a, Info 2): the ward's
//! pairing record is changed by the warden (a gift-wrapped RELAYS) and, on
//! Linux, by `charter-pair` in another process (a pasted relay code, a
//! re-pair). Each side loads, changes and saves the whole record; without a
//! lock one save can drop the other's change.
//!
//! `flock(2)` through std's `File::lock`: released when the guard drops or the
//! process dies, so a crash never leaves the record locked. Advisory: it
//! orders the writers that take it, which is every writer of the record.
//! Std only, so it builds under every `charter-sys` feature set.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// The lock file beside a pairing record (`<dir>/pairing.lock`).
pub const PAIRING_LOCK_FILE: &str = "pairing.lock";

/// A held exclusive lock; dropping it releases the lock.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

/// Take the exclusive lock on `path` (created `0600` if absent), blocking
/// until any other holder releases it.
pub fn lock_exclusive(path: &Path) -> std::io::Result<FileLock> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut open = OpenOptions::new();
    open.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let file = open.open(path)?;
    file.lock()?;
    Ok(FileLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_holder_waits_for_the_first() {
        let dir = std::env::temp_dir().join(format!("charter-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(PAIRING_LOCK_FILE);
        let first = lock_exclusive(&path).unwrap();
        let other = File::open(&path).unwrap();
        assert!(
            other.try_lock().is_err(),
            "held: a second lock must not be granted"
        );
        drop(first);
        assert!(other.try_lock().is_ok(), "released on drop");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
