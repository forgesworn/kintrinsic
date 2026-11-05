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
//!
//! # Two copies (D7)
//!
//! An unreadable state fails closed, and on Android the way out is a re-pair
//! or a factory reset. So the file store keeps the document twice, a primary
//! and a secondary, and every save writes both. The adapter reads the
//! primary, and the secondary only when the primary is missing, unreadable or
//! does not parse ([`GuardianStateStore::load_fallback`]): the state is lost
//! only to two independent corruptions. The second copy is no new trust
//! path: it is the same document, written by the same step.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::error::{SysError, SysResult};
use crate::secret_file::{read_secret, remove_secret, sibling, write_secret};

/// The state file's name in the ward's state directory.
pub const GUARDIAN_STATE_FILE: &str = "guardian-state.json";

/// The second copy's name beside it: [`GUARDIAN_STATE_FILE`] with `.2`
/// appended (no `.json` extension, so nothing that walks the directory for
/// records picks it up).
pub const GUARDIAN_STATE_SECONDARY_FILE: &str = "guardian-state.json.2";

/// The guardian-keys state document.
pub trait GuardianStateStore: Send + Sync {
    /// The primary copy, `None` when the ward has never written one. `Err`
    /// when it is there and cannot be read.
    fn load(&self) -> SysResult<Option<String>>;
    /// The secondary copy, read when the primary is missing, unreadable or
    /// does not parse. A store that keeps one copy has none (`Ok(None)`).
    fn load_fallback(&self) -> SysResult<Option<String>> {
        Ok(None)
    }
    /// Replace the document atomically, in every copy the store keeps. On
    /// `Err` the previous document is still the primary on disk.
    fn save(&self, json: &str) -> SysResult<()>;
    /// Forget it, every copy (an unpair).
    fn clear(&self) -> SysResult<()>;
}

/// The `0600` file store, with its second copy beside the first.
#[derive(Debug, Clone)]
pub struct FileGuardianStateStore {
    path: PathBuf,
    secondary: PathBuf,
}

impl FileGuardianStateStore {
    /// The primary at `path`, the secondary at `path` with `.2` appended.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let secondary = sibling(&path, ".2");
        FileGuardianStateStore { path, secondary }
    }

    /// [`GUARDIAN_STATE_FILE`] (and [`GUARDIAN_STATE_SECONDARY_FILE`]) inside
    /// `dir` (e.g. `/var/lib/charter`).
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self::new(dir.into().join(GUARDIAN_STATE_FILE))
    }

    /// The primary copy's path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The secondary copy's path.
    pub fn secondary_path(&self) -> &std::path::Path {
        &self.secondary
    }
}

fn io(ctx: &str, e: std::io::Error) -> SysError {
    SysError::Io(format!("{ctx}: {e}"))
}

impl GuardianStateStore for FileGuardianStateStore {
    fn load(&self) -> SysResult<Option<String>> {
        read_secret(&self.path).map_err(|e| io("read guardian state", e))
    }
    fn load_fallback(&self) -> SysResult<Option<String>> {
        read_secret(&self.secondary).map_err(|e| io("read guardian state (second copy)", e))
    }
    /// The secondary first, then the primary, each atomic. A failure of the
    /// first leaves both as they were; a failure of the second leaves the
    /// primary as it was and the secondary one step AHEAD of it, never behind
    /// (a copy behind could bring back a key a removal took out). The
    /// adapter keeps the primary's state in memory on `Err`, and its next
    /// save rewrites both.
    fn save(&self, json: &str) -> SysResult<()> {
        write_secret(&self.secondary, json.as_bytes())
            .map_err(|e| io("write guardian state (second copy)", e))?;
        write_secret(&self.path, json.as_bytes()).map_err(|e| io("write guardian state", e))
    }
    fn clear(&self) -> SysResult<()> {
        let primary = remove_secret(&self.path).map_err(|e| io("remove guardian state", e));
        let secondary = remove_secret(&self.secondary)
            .map_err(|e| io("remove guardian state (second copy)", e));
        primary.and(secondary)
    }
}

#[derive(Debug, Default)]
struct Memory {
    doc: Option<String>,
    secondary: Option<String>,
    fail_primary_reads: bool,
    fail_secondary_reads: bool,
    fail_writes: bool,
    saves: usize,
}

/// An in-memory store for tests, with fault injection. Clones share one
/// document, so a test can "restart" by building a new adapter over a clone.
/// It keeps two copies, as the file store does.
#[derive(Debug, Clone, Default)]
pub struct MemoryGuardianStateStore(Arc<Mutex<Memory>>);

impl MemoryGuardianStateStore {
    pub fn new() -> Self {
        Self::default()
    }
    /// Make every read of either copy fail (`true`) or work again.
    pub fn break_reads(&self, broken: bool) {
        let mut g = self.0.lock().expect("lock");
        g.fail_primary_reads = broken;
        g.fail_secondary_reads = broken;
    }
    /// Make reads of the primary copy alone fail (`true`) or work again.
    pub fn break_primary_reads(&self, broken: bool) {
        self.0.lock().expect("lock").fail_primary_reads = broken;
    }
    /// Make every `save` and `clear` fail (`true`) or work again.
    pub fn break_writes(&self, broken: bool) {
        self.0.lock().expect("lock").fail_writes = broken;
    }
    /// The primary copy as stored.
    pub fn raw(&self) -> Option<String> {
        self.0.lock().expect("lock").doc.clone()
    }
    /// The secondary copy as stored.
    pub fn raw_secondary(&self) -> Option<String> {
        self.0.lock().expect("lock").secondary.clone()
    }
    /// Put a document in place directly (a corrupt one, say), in both
    /// copies.
    pub fn set_raw(&self, doc: Option<String>) {
        let mut g = self.0.lock().expect("lock");
        g.secondary = doc.clone();
        g.doc = doc;
    }
    /// Put a primary copy in place directly, leaving the secondary.
    pub fn set_raw_primary(&self, doc: Option<String>) {
        self.0.lock().expect("lock").doc = doc;
    }
    /// Put a secondary copy in place directly, leaving the primary.
    pub fn set_raw_secondary(&self, doc: Option<String>) {
        self.0.lock().expect("lock").secondary = doc;
    }
    /// How many successful saves there have been.
    pub fn saves(&self) -> usize {
        self.0.lock().expect("lock").saves
    }
}

impl GuardianStateStore for MemoryGuardianStateStore {
    fn load(&self) -> SysResult<Option<String>> {
        let g = self.0.lock().expect("lock");
        if g.fail_primary_reads {
            return Err(SysError::Io("guardian state unreadable (injected)".into()));
        }
        Ok(g.doc.clone())
    }
    fn load_fallback(&self) -> SysResult<Option<String>> {
        let g = self.0.lock().expect("lock");
        if g.fail_secondary_reads {
            return Err(SysError::Io(
                "guardian state second copy unreadable (injected)".into(),
            ));
        }
        Ok(g.secondary.clone())
    }
    fn save(&self, json: &str) -> SysResult<()> {
        let mut g = self.0.lock().expect("lock");
        if g.fail_writes {
            return Err(SysError::Io("guardian state unwritable (injected)".into()));
        }
        g.doc = Some(json.to_owned());
        g.secondary = Some(json.to_owned());
        g.saves += 1;
        Ok(())
    }
    fn clear(&self) -> SysResult<()> {
        let mut g = self.0.lock().expect("lock");
        if g.fail_writes {
            return Err(SysError::Io("guardian state unwritable (injected)".into()));
        }
        g.doc = None;
        g.secondary = None;
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

    /// Every save writes both copies, `0600`; a clear removes both.
    #[test]
    fn the_file_store_keeps_a_second_copy() {
        let d = std::env::temp_dir().join(format!("charter-gstate2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = FileGuardianStateStore::in_dir(&d);
        assert_eq!(s.secondary_path(), d.join(GUARDIAN_STATE_SECONDARY_FILE));
        assert_eq!(s.load_fallback().unwrap(), None);
        s.save("{\"v\":1}").unwrap();
        s.save("{\"v\":2}").unwrap();
        assert_eq!(s.load().unwrap().as_deref(), Some("{\"v\":2}"));
        assert_eq!(s.load_fallback().unwrap().as_deref(), Some("{\"v\":2}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(s.secondary_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        // The primary lost: the secondary is still there.
        std::fs::remove_file(s.path()).unwrap();
        assert_eq!(s.load().unwrap(), None);
        assert_eq!(s.load_fallback().unwrap().as_deref(), Some("{\"v\":2}"));
        s.clear().unwrap();
        assert_eq!(s.load().unwrap(), None);
        assert_eq!(s.load_fallback().unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
