//! Small persisted JSON stores under the link's state directory. Every write
//! is atomic (temp file, fsync, rename) and owner-only (0600 on Unix). A
//! missing file is an empty store; a corrupt one is logged and treated as
//! empty, so a bad file never stops the link from starting.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Current unix time in seconds (0 if the clock is before the epoch).
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lock a std mutex, recovering from poisoning: a panicked holder must not
/// take the whole link down with it.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read and parse `path`; `None` when missing or unreadable.
pub(crate) fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("charter-link: cannot read {}: {e}", file_name(path));
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!(
                "charter-link: {} is corrupt; starting empty",
                file_name(path)
            );
            None
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Atomically replace `path` with `value` as JSON, owner-only.
pub(crate) fn save<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("state path has no parent"))?;
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_name);
    // A stale temp file from a crash may carry other permissions.
    let _ = fs::remove_file(&tmp);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f = opts.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// [`save`], logging rather than returning a failure: persistence is best
/// effort and the in-memory state stays authoritative for this run.
pub(crate) fn save_logged<T: Serialize>(path: &Path, value: &T) {
    if let Err(e) = save(path, value) {
        eprintln!("charter-link: cannot write {}: {e}", file_name(path));
    }
}
