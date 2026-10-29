//! Files only the ward's own process may read: the presence token and the
//! guardian-keys state (which holds the guardian PIN record). Each is written
//! through a fresh `0600` temp file and a rename, so it is never readable by
//! another user, not even for the length of a write, and a crash never leaves
//! it half-written. Std only, so it builds under every `charter-sys` feature
//! set (the Linux warden and the Android ward share it).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `body` to `path` via a fresh `0600` temp file and a rename. The mode
/// is set AT OPEN on a file this call created: a write-then-chmod would leave
/// the file readable under a 022 umask for the length of the write. The
/// temp file is unique to this call (process id and a counter), so two
/// writers never rename each other's half-written file into place (review
/// batch 2, core Info 8); a leftover of the same name is removed first so
/// `create_new` never inherits an old file's mode or an open reader. The temp
/// file is fsynced before the rename, and the directory after it, so the
/// rename itself survives a power cut (review batch 2, core L2: a PIN counter
/// or an applied departure must not roll back on a hard reset).
pub fn write_secret(path: &Path, body: &[u8]) -> std::io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = sibling(path, &format!(".{}.{n}.tmp", std::process::id()));
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let written = write_tmp(&tmp, body).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // The new file is in place: a failed directory sync only weakens its
    // durability, and reporting the write as failed would leave the caller's
    // memory behind what is on disk.
    if let Err(e) = sync_parent(path) {
        eprintln!(
            "charter: could not sync {} after a write ({e})",
            path.display()
        );
    }
    Ok(())
}

/// Make a rename in `path`'s directory durable (Unix: fsync the directory).
fn sync_parent(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_tmp(tmp: &Path, body: &[u8]) -> std::io::Result<()> {
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(body)?;
        f.sync_all()?;
    }
    Ok(())
}

/// Read `path` to a string; a missing file is `None`, any other failure an
/// error (the caller decides what an unreadable secret means).
pub fn read_secret(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Remove `path`; a missing file is not an error.
pub fn remove_secret(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_secret_is_0600_and_replaced_whole() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("charter-secret-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("s.json");
        assert_eq!(read_secret(&p).unwrap(), None);
        write_secret(&p, b"one").unwrap();
        write_secret(&p, b"two").unwrap();
        assert_eq!(read_secret(&p).unwrap().as_deref(), Some("two"));
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        remove_secret(&p).unwrap();
        remove_secret(&p).unwrap();
        assert_eq!(read_secret(&p).unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
