//! Where a ward keeps its guardian-keys state (contract, *Guardian roots and
//! working keys (v1)*): one opaque JSON document, owned and shaped by
//! `charter-spine`'s guardian-keys adapter.
//!
//! The document is a secret. It holds the guardian PIN record (`{ salt, hash
//! }` of a 4–6 digit PIN falls to an offline search) and the live presence
//! tokens, so the file store writes it `0600`, atomically (rule 10: a ward
//! persists a statement before it applies it, and a torn write must never be
//! read back as some other state). Std only: the Linux warden and the Android
//! ward share it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::error::{SysError, SysResult};
use crate::secret_file::{read_secret, remove_secret, write_secret};

/// The state file's name in the ward's state directory.
pub const GUARDIAN_STATE_FILE: &str = "guardian-state.json";

/// The guardian-keys state document.
pub trait GuardianStateStore: Send + Sync {
    /// The document, `None` when the ward has never written one. `Err` when
    /// it is there and cannot be read: the adapter fails closed on that.
    fn load(&self) -> SysResult<Option<String>>;
    /// Replace the document atomically. On `Err` the previous document is
    /// still the one on disk.
    fn save(&self, json: &str) -> SysResult<()>;
    /// Forget it (an unpair).
    fn clear(&self) -> SysResult<()>;
}

/// The `0600` file store.
#[derive(Debug, Clone)]
pub struct FileGuardianStateStore {
    path: PathBuf,
}

impl FileGuardianStateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        FileGuardianStateStore { path: path.into() }
    }

    /// [`GUARDIAN_STATE_FILE`] inside `dir` (e.g. `/var/lib/charter`).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self::new(dir.into().join(GUARDIAN_STATE_FILE))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn io(ctx: &str, e: std::io::Error) -> SysError {
    SysError::Io(format!("{ctx}: {e}"))
}

impl GuardianStateStore for FileGuardianStateStore {
    fn load(&self) -> SysResult<Option<String>> {
        read_secret(&self.path).map_err(|e| io("read guardian state", e))
    }
    fn save(&self, json: &str) -> SysResult<()> {
        write_secret(&self.path, json.as_bytes()).map_err(|e| io("write guardian state", e))
    }
    fn clear(&self) -> SysResult<()> {
        remove_secret(&self.path).map_err(|e| io("remove guardian state", e))
    }
}

#[derive(Debug, Default)]
struct Memory {
    doc: Option<String>,
    fail_reads: bool,
    fail_writes: bool,
    saves: usize,
}

/// An in-memory store for tests, with fault injection. Clones share one
/// document, so a test can "restart" by building a new adapter over a clone.
#[derive(Debug, Clone, Default)]
pub struct MemoryGuardianStateStore(Arc<Mutex<Memory>>);

impl MemoryGuardianStateStore {
    pub fn new() -> Self {
        Self::default()
    }
    /// Make every `load` fail (`true`) or work again.
    pub fn break_reads(&self, broken: bool) {
        self.0.lock().expect("lock").fail_reads = broken;
    }
    /// Make every `save` and `clear` fail (`true`) or work again.
    pub fn break_writes(&self, broken: bool) {
        self.0.lock().expect("lock").fail_writes = broken;
    }
    /// The document as stored.
    pub fn raw(&self) -> Option<String> {
        self.0.lock().expect("lock").doc.clone()
    }
    /// Put a document in place directly (a corrupt one, say).
    pub fn set_raw(&self, doc: Option<String>) {
        self.0.lock().expect("lock").doc = doc;
    }
    /// How many successful saves there have been.
    pub fn saves(&self) -> usize {
        self.0.lock().expect("lock").saves
    }
}

impl GuardianStateStore for MemoryGuardianStateStore {
    fn load(&self) -> SysResult<Option<String>> {
        let g = self.0.lock().expect("lock");
        if g.fail_reads {
            return Err(SysError::Io("guardian state unreadable (injected)".into()));
        }
        Ok(g.doc.clone())
    }
    fn save(&self, json: &str) -> SysResult<()> {
        let mut g = self.0.lock().expect("lock");
        if g.fail_writes {
            return Err(SysError::Io("guardian state unwritable (injected)".into()));
        }
        g.doc = Some(json.to_owned());
        g.saves += 1;
        Ok(())
    }
    fn clear(&self) -> SysResult<()> {
        let mut g = self.0.lock().expect("lock");
        if g.fail_writes {
            return Err(SysError::Io("guardian state unwritable (injected)".into()));
        }
        g.doc = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_store_round_trips_and_clears() {
        let d = std::env::temp_dir().join(format!("charter-gstate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = FileGuardianStateStore::in_dir(&d);
        assert_eq!(s.load().unwrap(), None);
        s.save("{\"v\":1}").unwrap();
        assert_eq!(s.load().unwrap().as_deref(), Some("{\"v\":1}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(s.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        s.clear().unwrap();
        assert_eq!(s.load().unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
