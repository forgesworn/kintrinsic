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
/// the file readable under a 022 umask for the length of the write. A
/// leftover temp file is removed first so `create_new` never inherits an old
/// file's mode or an open reader. The temp file is fsynced before the rename.
pub fn write_secret(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = sibling(path, ".tmp");
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
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
