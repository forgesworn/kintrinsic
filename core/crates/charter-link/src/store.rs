//! Small persisted JSON stores under the link's state directory. Every write
//! is atomic (temp file, fsync, rename, directory fsync) and owner-only
//! (0600 on Unix). A missing file is an empty store. A corrupt or unreadable
//! one is moved aside (`<name>.corrupt-<unix>`) and logged once, and the
//! store starts empty; a file that cannot be moved aside is never written,
//! so nothing is silently overwritten.
//!
//! Writes go through [`Persist`]: the caller stages a snapshot while it
//! holds its state lock (so the staged value is always the newest), then
//! writes it after releasing the lock. One writer per file at a time, and a
//! write always takes the newest staged snapshot, so concurrent changes
//! coalesce and an older snapshot never lands after a newer one.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// What [`load`] found.
pub(crate) struct Loaded<T> {
    /// The parsed store, `None` when missing, corrupt or unreadable.
    pub(crate) value: Option<T>,
    /// False when a bad file could not be moved aside: the store then runs
    /// in memory only rather than overwrite it.
    pub(crate) writable: bool,
}

/// Read and parse `path`. A missing file is `None`; a corrupt or unreadable
/// one is renamed to `<name>.corrupt-<unix>` first.
pub(crate) fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T> {
    let why = match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(v) => {
                return Loaded {
                    value: Some(v),
                    writable: true,
                }
            }
            Err(e) => format!("corrupt ({e})"),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded {
                value: None,
                writable: true,
            }
        }
        Err(e) => format!("unreadable ({e})"),
    };
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".corrupt-{}", now_unix()));
    let aside = PathBuf::from(aside);
    match fs::rename(path, &aside) {
        Ok(()) => {
            sync_parent(path);
            eprintln!(
                "charter-link: {} is {why}; moved aside to {} and starting empty",
                file_name(path),
                file_name(&aside)
            );
            Loaded {
                value: None,
                writable: true,
            }
        }
        Err(e) => {
            eprintln!(
                "charter-link: {} is {why} and cannot be moved aside ({e}); \
                 starting empty and not writing it this run",
                file_name(path)
            );
            Loaded {
                value: None,
                writable: false,
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Make a rename in `path`'s directory durable (best effort, Unix only).
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Atomically replace `path` with `value` as JSON, owner-only. Callers
/// must not run two of these on the same path at once ([`Persist`] makes
/// sure of that).
pub(crate) fn save<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("state path has no parent"))?;
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
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
    fs::rename(&tmp, path)?;
    sync_parent(path);
    Ok(())
}

/// [`save`], logging rather than returning a failure: persistence is best
/// effort and the in-memory state stays authoritative for this run.
pub(crate) fn save_logged<T: Serialize + ?Sized>(path: &Path, value: &T) {
    if let Err(e) = save(path, value) {
        eprintln!("charter-link: cannot write {}: {e}", file_name(path));
    }
}

/// One persisted file: a staged snapshot and a single writer.
pub(crate) struct Persist<T> {
    path: PathBuf,
    /// The newest snapshot not yet written.
    pending: Mutex<Option<T>>,
    /// Held for the whole write; `true` once closed (no further writes).
    writer: Mutex<bool>,
}

impl<T: Serialize + Send + 'static> Persist<T> {
    /// A store for `path`; `writable: false` (from [`Loaded`]) never writes.
    pub(crate) fn new(path: PathBuf, writable: bool) -> Arc<Self> {
        Arc::new(Persist {
            path,
            pending: Mutex::new(None),
            writer: Mutex::new(!writable),
        })
    }

    /// Record `value` as the newest snapshot. Call this while holding the
    /// lock that guards the state it was taken from, then [`Persist::flush`]
    /// after releasing it.
    pub(crate) fn stage(&self, value: T) {
        *lock(&self.pending) = Some(value);
    }

    /// Write the newest staged snapshot, if any. Blocking.
    pub(crate) fn flush(&self) {
        let closed = lock(&self.writer);
        if *closed {
            return;
        }
        let next = lock(&self.pending).take();
        if let Some(v) = next {
            save_logged(&self.path, &v);
        }
        drop(closed);
    }

    /// [`Persist::flush`] on tokio's blocking pool, from async code. The
    /// write completes even if the awaiting task is cancelled.
    pub(crate) async fn flush_async(self: &Arc<Self>) {
        let me = self.clone();
        let _ = tokio::task::spawn_blocking(move || me.flush()).await;
    }

    /// Start a [`Persist::flush`] on `rt`'s blocking pool without waiting.
    pub(crate) fn flush_in_background(self: &Arc<Self>, rt: &tokio::runtime::Handle) {
        let me = self.clone();
        drop(rt.spawn_blocking(move || me.flush()));
    }

    /// Write whatever is staged, wait for any write in flight, and refuse
    /// every later write: after this returns the file is final.
    pub(crate) fn close(&self) {
        let mut closed = lock(&self.writer);
        if *closed {
            return;
        }
        let next = lock(&self.pending).take();
        if let Some(v) = next {
            save_logged(&self.path, &v);
        }
        *closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_file_is_moved_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        fs::write(&path, b"{\"v\":1,TRUNCATED").unwrap();
        let l = load::<serde_json::Value>(&path);
        assert!(l.value.is_none());
        assert!(l.writable);
        assert!(!path.exists());
        let aside: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("x.json.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            fs::read(dir.path().join(&aside[0])).unwrap(),
            b"{\"v\":1,TRUNCATED"
        );
    }

    #[test]
    fn a_missing_file_is_empty_and_writable() {
        let dir = tempfile::tempdir().unwrap();
        let l = load::<serde_json::Value>(&dir.path().join("none.json"));
        assert!(l.value.is_none() && l.writable);
    }

    #[test]
    fn persist_writes_the_newest_snapshot_and_nothing_after_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let p = Persist::<u32>::new(path.clone(), true);
        p.stage(1);
        p.stage(2);
        p.flush();
        assert_eq!(fs::read_to_string(&path).unwrap(), "2");
        p.flush(); // nothing staged: no rewrite
        p.stage(3);
        p.close();
        assert_eq!(fs::read_to_string(&path).unwrap(), "3");
        p.stage(4);
        p.flush();
        assert_eq!(fs::read_to_string(&path).unwrap(), "3");
    }

    #[test]
    fn an_unwritable_store_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let p = Persist::<u32>::new(path.clone(), false);
        p.stage(1);
        p.flush();
        p.close();
        assert!(!path.exists());
    }
}
