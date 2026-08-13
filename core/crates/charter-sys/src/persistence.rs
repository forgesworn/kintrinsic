//! Named, typed persistence ports. These hold the daemon's durable state:
//! consumed request ids (single-use replay defense), pending requests,
//! authenticated standing clauses (with per-kind rollback protection), usage
//! and extension snapshots, and the guardian pairing.
//!
//! The mock impls back onto a shared [`MockDisk`] so "reopening" a store (a new
//! instance over the same disk handle) sees prior writes — modelling
//! restart-durability without touching the filesystem.

use charter_primitives::ReqId;

use crate::error::SysResult;

/// Single-use consumed-id store. `check_and_consume` is atomic: it records the
/// id and returns whether it was *newly* consumed (true) or a replay (false).
/// A consumed id is retained at least until its grant `exp`.
pub trait ConsumedIdStore: Send + Sync {
    /// Atomically record `req_id` (retained until `exp`). Returns true if newly
    /// consumed, false if already present (a replay).
    fn check_and_consume(&self, req_id: &ReqId, exp: u64) -> SysResult<bool>;
    /// Drop entries whose `exp` is at or before `now`.
    fn purge_expired(&self, now: u64) -> SysResult<()>;
}

/// Pending-request store (JSON blob per req id).
pub trait PendingStore: Send + Sync {
    /// Upsert the pending record JSON under `req_id`.
    fn put(&self, req_id: &str, json: &str) -> SysResult<()>;
    /// All pending records as `(req_id, json)`.
    fn list(&self) -> SysResult<Vec<(String, String)>>;
    /// Remove the record for `req_id` (no-op if absent).
    fn remove(&self, req_id: &str) -> SysResult<()>;
}

/// Authenticated standing-clause store with per-kind monotonic `issuedAt`
/// rollback protection. `put_clause` rejects a clause whose `issued_at` is at
/// or below the highest already seen for that kind.
pub trait ClauseStore: Send + Sync {
    /// Store an authenticated clause. Returns true if stored, false if rejected
    /// as a rollback (`issued_at <= highest seen for kind`).
    fn put_clause(&self, kind: u16, issued_at: u64, json: &str) -> SysResult<bool>;
    /// The latest stored clause JSON for `kind`.
    fn get_clause(&self, kind: u16) -> SysResult<Option<String>>;
    /// The highest `issued_at` ever accepted for `kind`.
    fn highest_issued_at(&self, kind: u16) -> SysResult<Option<u64>>;
}

/// Cache of signature-verified curator web lists, keyed by an opaque string
/// (`"<curator-hex>:<listId>"`). Like [`ClauseStore`], each key is rollback-
/// protected by a monotonic `created_at`: a stale replacement is rejected, so a
/// hostile relay replaying an old list cannot revert the cache. `all_lists`
/// returns the newest JSON for every cached key — the evaluator's input.
pub trait CuratorListStore: Send + Sync {
    /// Upsert `json` under `key`. Returns true if stored, false if rejected as a
    /// rollback (`created_at <= highest seen for key`).
    fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool>;
    /// Every cached list's JSON, in deterministic (key) order.
    fn all_lists(&self) -> SysResult<Vec<String>>;
}

/// Per-child authenticated standing-clause store: like [`ClauseStore`] but keyed
/// by `(subject_hex, kind)` so a guardian can carry independent `schedule`/
/// `budget` clauses for **each** managed child (multi-child, Signet-first). Each
/// `(subject, kind)` slot is independently rollback-protected by a monotonic
/// `issued_at`, so a hostile relay can neither revert one child's clause nor
/// replay another child's into theirs. `clauses_for` returns every cached
/// `(kind, json)` for one subject — the per-child policy resolver's input.
pub trait ChildClauseStore: Send + Sync {
    /// Store an authenticated clause for `(subject_hex, kind)`. Returns true if
    /// stored, false if rejected as a rollback (`issued_at <= highest seen for
    /// that (subject, kind)`).
    fn put_child_clause(
        &self,
        subject_hex: &str,
        kind: u16,
        issued_at: u64,
        json: &str,
    ) -> SysResult<bool>;
    /// The latest stored clause JSON for `(subject_hex, kind)`.
    fn get_child_clause(&self, subject_hex: &str, kind: u16) -> SysResult<Option<String>>;
    /// The highest `issued_at` ever accepted for `(subject_hex, kind)`.
    fn highest_issued_at(&self, subject_hex: &str, kind: u16) -> SysResult<Option<u64>>;
    /// Every cached `(kind, json)` for `subject_hex`, in ascending `kind` order.
    fn clauses_for(&self, subject_hex: &str) -> SysResult<Vec<(u16, String)>>;
    /// Forget every clause for `subject_hex` (a release: enforcement must stop,
    /// and the rollback floor must reset so a later re-pair starts clean).
    fn clear_for(&self, subject_hex: &str) -> SysResult<()>;
}

/// Usage-accounting snapshot store (opaque JSON; shape owned by Phase 6).
pub trait UsageStore: Send + Sync {
    fn save_snapshot(&self, json: &str) -> SysResult<()>;
    fn load_snapshot(&self) -> SysResult<Option<String>>;
}

/// Extension-ledger snapshot store (opaque JSON; shape owned by Phase 6/7).
pub trait ExtensionStore: Send + Sync {
    fn save_snapshot(&self, json: &str) -> SysResult<()>;
    fn load_snapshot(&self) -> SysResult<Option<String>>;
}

/// Guardian-pairing store (opaque JSON; shape owned by Phase 2).
pub trait PairingStore: Send + Sync {
    fn save(&self, json: &str) -> SysResult<()>;
    fn load(&self) -> SysResult<Option<String>>;
    /// Forget the pairing entirely (a parent-gated release / unpair).
    fn clear(&self) -> SysResult<()>;
}

// ---------------------------------------------------------------------------
// Mock impls (shared in-memory disk).
// ---------------------------------------------------------------------------

#[cfg(feature = "mock")]
mod mock {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use charter_primitives::ReqId;

    use super::*;

    #[derive(Default)]
    struct DiskState {
        consumed: HashMap<[u8; 32], u64>,
        pending: BTreeMap<String, String>,
        clauses: HashMap<u16, (u64, String)>,
        clause_high: HashMap<u16, u64>,
        // Per-(subject_hex, kind) authenticated clauses. The stored `issued_at`
        // is itself the per-slot high-water mark (rollback bound).
        child_clauses: BTreeMap<(String, u16), (u64, String)>,
        curator_lists: BTreeMap<String, (u64, String)>,
        usage: Option<String>,
        extension: Option<String>,
        pairing: Option<String>,
    }

    /// A shared in-memory "disk". Cloning shares the same backing state, so a
    /// fresh store over a cloned handle sees prior writes (restart-durability).
    #[derive(Clone, Default)]
    pub struct MockDisk(Arc<Mutex<DiskState>>);

    impl MockDisk {
        /// A new empty disk.
        pub fn new() -> Self {
            Self::default()
        }
    }

    /// Mock consumed-id store.
    pub struct MockConsumedIdStore {
        disk: MockDisk,
    }
    impl MockConsumedIdStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl ConsumedIdStore for MockConsumedIdStore {
        fn check_and_consume(&self, req_id: &ReqId, exp: u64) -> SysResult<bool> {
            let mut g = self.disk.0.lock().expect("disk lock");
            let key = *req_id.as_bytes();
            if g.consumed.contains_key(&key) {
                return Ok(false);
            }
            g.consumed.insert(key, exp);
            Ok(true)
        }
        fn purge_expired(&self, now: u64) -> SysResult<()> {
            let mut g = self.disk.0.lock().expect("disk lock");
            g.consumed.retain(|_, &mut exp| exp > now);
            Ok(())
        }
    }

    /// Mock pending store.
    pub struct MockPendingStore {
        disk: MockDisk,
    }
    impl MockPendingStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl PendingStore for MockPendingStore {
        fn put(&self, req_id: &str, json: &str) -> SysResult<()> {
            self.disk
                .0
                .lock()
                .expect("disk lock")
                .pending
                .insert(req_id.to_string(), json.to_string());
            Ok(())
        }
        fn list(&self) -> SysResult<Vec<(String, String)>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .pending
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
        fn remove(&self, req_id: &str) -> SysResult<()> {
            self.disk
                .0
                .lock()
                .expect("disk lock")
                .pending
                .remove(req_id);
            Ok(())
        }
    }

    /// Mock clause store with rollback protection.
    pub struct MockClauseStore {
        disk: MockDisk,
    }
    impl MockClauseStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl ClauseStore for MockClauseStore {
        fn put_clause(&self, kind: u16, issued_at: u64, json: &str) -> SysResult<bool> {
            let mut g = self.disk.0.lock().expect("disk lock");
            if let Some(&high) = g.clause_high.get(&kind) {
                if issued_at <= high {
                    return Ok(false);
                }
            }
            g.clause_high.insert(kind, issued_at);
            g.clauses.insert(kind, (issued_at, json.to_string()));
            Ok(true)
        }
        fn get_clause(&self, kind: u16) -> SysResult<Option<String>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .clauses
                .get(&kind)
                .map(|(_, j)| j.clone()))
        }
        fn highest_issued_at(&self, kind: u16) -> SysResult<Option<u64>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .clause_high
                .get(&kind)
                .copied())
        }
    }

    /// Mock curator-list store with per-key rollback protection.
    pub struct MockCuratorListStore {
        disk: MockDisk,
    }
    impl MockCuratorListStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl CuratorListStore for MockCuratorListStore {
        fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool> {
            let mut g = self.disk.0.lock().expect("disk lock");
            if let Some((prev, _)) = g.curator_lists.get(key) {
                if created_at <= *prev {
                    return Ok(false);
                }
            }
            g.curator_lists
                .insert(key.to_string(), (created_at, json.to_string()));
            Ok(true)
        }
        fn all_lists(&self) -> SysResult<Vec<String>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .curator_lists
                .values()
                .map(|(_, j)| j.clone())
                .collect())
        }
    }

    /// Mock per-child clause store with per-(subject, kind) rollback protection.
    pub struct MockChildClauseStore {
        disk: MockDisk,
    }
    impl MockChildClauseStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl ChildClauseStore for MockChildClauseStore {
        fn put_child_clause(
            &self,
            subject_hex: &str,
            kind: u16,
            issued_at: u64,
            json: &str,
        ) -> SysResult<bool> {
            let mut g = self.disk.0.lock().expect("disk lock");
            let key = (subject_hex.to_string(), kind);
            if let Some((prev, _)) = g.child_clauses.get(&key) {
                if issued_at <= *prev {
                    return Ok(false);
                }
            }
            g.child_clauses.insert(key, (issued_at, json.to_string()));
            Ok(true)
        }
        fn get_child_clause(&self, subject_hex: &str, kind: u16) -> SysResult<Option<String>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .child_clauses
                .get(&(subject_hex.to_string(), kind))
                .map(|(_, j)| j.clone()))
        }
        fn highest_issued_at(&self, subject_hex: &str, kind: u16) -> SysResult<Option<u64>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .child_clauses
                .get(&(subject_hex.to_string(), kind))
                .map(|(t, _)| *t))
        }
        fn clauses_for(&self, subject_hex: &str) -> SysResult<Vec<(u16, String)>> {
            // BTreeMap is ordered by (subject, kind), so a subject's entries come
            // out in ascending `kind` already.
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .child_clauses
                .iter()
                .filter(|((s, _), _)| s == subject_hex)
                .map(|((_, k), (_, j))| (*k, j.clone()))
                .collect())
        }
        fn clear_for(&self, subject_hex: &str) -> SysResult<()> {
            self.disk
                .0
                .lock()
                .expect("disk lock")
                .child_clauses
                .retain(|(s, _), _| s != subject_hex);
            Ok(())
        }
    }

    macro_rules! blob_store {
        ($store:ident, $field:ident, $trait:ident) => {
            pub struct $store {
                disk: MockDisk,
            }
            impl $store {
                pub fn new(disk: MockDisk) -> Self {
                    Self { disk }
                }
            }
            impl $trait for $store {
                fn save_snapshot(&self, json: &str) -> SysResult<()> {
                    self.disk.0.lock().expect("disk lock").$field = Some(json.to_string());
                    Ok(())
                }
                fn load_snapshot(&self) -> SysResult<Option<String>> {
                    Ok(self.disk.0.lock().expect("disk lock").$field.clone())
                }
            }
        };
    }
    blob_store!(MockUsageStore, usage, UsageStore);
    blob_store!(MockExtensionStore, extension, ExtensionStore);

    /// Mock pairing store.
    pub struct MockPairingStore {
        disk: MockDisk,
    }
    impl MockPairingStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl PairingStore for MockPairingStore {
        fn save(&self, json: &str) -> SysResult<()> {
            self.disk.0.lock().expect("disk lock").pairing = Some(json.to_string());
            Ok(())
        }
        fn load(&self) -> SysResult<Option<String>> {
            Ok(self.disk.0.lock().expect("disk lock").pairing.clone())
        }
        fn clear(&self) -> SysResult<()> {
            self.disk.0.lock().expect("disk lock").pairing = None;
            Ok(())
        }
    }
}

#[cfg(feature = "mock")]
pub use mock::{
    MockChildClauseStore, MockClauseStore, MockConsumedIdStore, MockCuratorListStore, MockDisk,
    MockExtensionStore, MockPairingStore, MockPendingStore, MockUsageStore,
};

// ---------------------------------------------------------------------------
// Real stubs (compile-only).
// ---------------------------------------------------------------------------

#[cfg(any(feature = "real-relay", feature = "real-os"))]
mod real {
    //! On-disk persistence rooted at `/var/lib/charter` (override via `with_base`
    //! in tests). Every mutating write is atomic — temp file + fsync + rename —
    //! so a crash mid-write never leaves a half-written file; the read-modify-
    //! write ports take an in-process lock so concurrent daemon tasks cannot
    //! race a check against a write. These are headless-runnable (pure
    //! filesystem), so unlike the OS-effect ports they are verified by the
    //! `#[cfg(test)]` suite below under `--features real`.

    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use charter_primitives::ReqId;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::fsutil::{
        atomic_write, hex_bytes, io_err, is_json, read_json, read_opt, write_json,
    };

    const DEFAULT_BASE: &str = "/var/lib/charter";

    // ---- consumed ids -----------------------------------------------------

    pub struct RealConsumedIdStore {
        base: PathBuf,
        lock: Mutex<()>,
    }
    impl Default for RealConsumedIdStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealConsumedIdStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                lock: Mutex::new(()),
            }
        }
        fn path(&self) -> PathBuf {
            self.base.join("consumed.json")
        }
        fn load(&self) -> SysResult<BTreeMap<String, u64>> {
            Ok(read_json(&self.path())?.unwrap_or_default())
        }
    }
    impl ConsumedIdStore for RealConsumedIdStore {
        fn check_and_consume(&self, req_id: &ReqId, exp: u64) -> SysResult<bool> {
            let _g = self.lock.lock().expect("consumed lock");
            let mut map = self.load()?;
            let key = hex_bytes(req_id.as_bytes());
            if map.contains_key(&key) {
                return Ok(false);
            }
            map.insert(key, exp);
            write_json(&self.path(), &map)?;
            Ok(true)
        }
        fn purge_expired(&self, now: u64) -> SysResult<()> {
            let _g = self.lock.lock().expect("consumed lock");
            let mut map = self.load()?;
            let before = map.len();
            map.retain(|_, &mut exp| exp > now);
            if map.len() != before {
                write_json(&self.path(), &map)?;
            }
            Ok(())
        }
    }

    // ---- pending requests -------------------------------------------------

    #[derive(Serialize, Deserialize)]
    struct PendingRec {
        id: String,
        json: String,
    }

    pub struct RealPendingStore {
        base: PathBuf,
    }
    impl Default for RealPendingStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealPendingStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self { base: base.into() }
        }
        fn dir(&self) -> PathBuf {
            self.base.join("pending")
        }
        fn file(&self, req_id: &str) -> PathBuf {
            self.dir()
                .join(format!("{}.json", hex_bytes(req_id.as_bytes())))
        }
    }
    impl PendingStore for RealPendingStore {
        fn put(&self, req_id: &str, json: &str) -> SysResult<()> {
            write_json(
                &self.file(req_id),
                &PendingRec {
                    id: req_id.to_string(),
                    json: json.to_string(),
                },
            )
        }
        fn list(&self) -> SysResult<Vec<(String, String)>> {
            let rd = match fs::read_dir(self.dir()) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(io_err("read_dir pending", e)),
            };
            let mut out = vec![];
            for entry in rd {
                let path = entry.map_err(|e| io_err("dir entry", e))?.path();
                if !is_json(&path) {
                    continue;
                }
                if let Some(rec) = read_json::<PendingRec>(&path)? {
                    out.push((rec.id, rec.json));
                }
            }
            out.sort();
            Ok(out)
        }
        fn remove(&self, req_id: &str) -> SysResult<()> {
            match fs::remove_file(self.file(req_id)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove pending", e)),
            }
        }
    }

    // ---- clauses ----------------------------------------------------------

    #[derive(Serialize, Deserialize)]
    struct ClauseRec {
        issued_at: u64,
        json: String,
    }

    pub struct RealClauseStore {
        base: PathBuf,
        lock: Mutex<()>,
    }
    impl Default for RealClauseStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealClauseStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                lock: Mutex::new(()),
            }
        }
        fn file(&self, kind: u16) -> PathBuf {
            self.base.join("clauses").join(format!("{kind}.json"))
        }
        fn current(&self, kind: u16) -> SysResult<Option<ClauseRec>> {
            read_json(&self.file(kind))
        }
    }
    impl ClauseStore for RealClauseStore {
        fn put_clause(&self, kind: u16, issued_at: u64, json: &str) -> SysResult<bool> {
            let _g = self.lock.lock().expect("clause lock");
            if let Some(cur) = self.current(kind)? {
                if issued_at <= cur.issued_at {
                    return Ok(false);
                }
            }
            write_json(
                &self.file(kind),
                &ClauseRec {
                    issued_at,
                    json: json.to_string(),
                },
            )?;
            Ok(true)
        }
        fn get_clause(&self, kind: u16) -> SysResult<Option<String>> {
            Ok(self.current(kind)?.map(|r| r.json))
        }
        fn highest_issued_at(&self, kind: u16) -> SysResult<Option<u64>> {
            Ok(self.current(kind)?.map(|r| r.issued_at))
        }
    }

    // ---- per-child clauses ------------------------------------------------

    /// On-disk per-child clauses at `<base>/children/<subject_hex>/clauses/<kind>.json`
    /// (reusing [`ClauseRec`]). Each `(subject, kind)` file is independently
    /// rollback-protected. `subject_hex` is sanitised to lowercase hex chars so
    /// it can never escape the base via path separators.
    pub struct RealChildClauseStore {
        base: PathBuf,
        lock: Mutex<()>,
    }
    impl Default for RealChildClauseStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealChildClauseStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                lock: Mutex::new(()),
            }
        }
        /// Keep only lowercase hex so a crafted `subject_hex` can never traverse
        /// out of the base dir (defence in depth; the broker only ever passes a
        /// verified pubkey hex).
        fn safe(subject_hex: &str) -> String {
            subject_hex
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .map(|c| c.to_ascii_lowercase())
                .collect()
        }
        fn clauses_dir(&self, subject_hex: &str) -> PathBuf {
            self.base
                .join("children")
                .join(Self::safe(subject_hex))
                .join("clauses")
        }
        fn file(&self, subject_hex: &str, kind: u16) -> PathBuf {
            self.clauses_dir(subject_hex).join(format!("{kind}.json"))
        }
        fn current(&self, subject_hex: &str, kind: u16) -> SysResult<Option<ClauseRec>> {
            read_json(&self.file(subject_hex, kind))
        }
    }
    impl ChildClauseStore for RealChildClauseStore {
        fn put_child_clause(
            &self,
            subject_hex: &str,
            kind: u16,
            issued_at: u64,
            json: &str,
        ) -> SysResult<bool> {
            let _g = self.lock.lock().expect("child clause lock");
            if let Some(cur) = self.current(subject_hex, kind)? {
                if issued_at <= cur.issued_at {
                    return Ok(false);
                }
            }
            write_json(
                &self.file(subject_hex, kind),
                &ClauseRec {
                    issued_at,
                    json: json.to_string(),
                },
            )?;
            Ok(true)
        }
        fn get_child_clause(&self, subject_hex: &str, kind: u16) -> SysResult<Option<String>> {
            Ok(self.current(subject_hex, kind)?.map(|r| r.json))
        }
        fn highest_issued_at(&self, subject_hex: &str, kind: u16) -> SysResult<Option<u64>> {
            Ok(self.current(subject_hex, kind)?.map(|r| r.issued_at))
        }
        fn clauses_for(&self, subject_hex: &str) -> SysResult<Vec<(u16, String)>> {
            let rd = match fs::read_dir(self.clauses_dir(subject_hex)) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(io_err("read_dir child clauses", e)),
            };
            let mut out = vec![];
            for entry in rd {
                let path = entry.map_err(|e| io_err("dir entry", e))?.path();
                if !is_json(&path) {
                    continue;
                }
                let Some(kind) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u16>().ok())
                else {
                    continue;
                };
                if let Some(rec) = read_json::<ClauseRec>(&path)? {
                    out.push((kind, rec.json));
                }
            }
            out.sort_by_key(|(k, _)| *k);
            Ok(out)
        }
        fn clear_for(&self, subject_hex: &str) -> SysResult<()> {
            let _g = self.lock.lock().expect("child clause lock");
            match fs::remove_dir_all(self.clauses_dir(subject_hex)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove child clauses", e)),
            }
        }
    }

    // ---- curator lists ----------------------------------------------------

    #[derive(Serialize, Deserialize)]
    struct CuratorRec {
        key: String,
        created_at: u64,
        json: String,
    }

    pub struct RealCuratorListStore {
        base: PathBuf,
        lock: Mutex<()>,
    }
    impl Default for RealCuratorListStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealCuratorListStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                lock: Mutex::new(()),
            }
        }
        fn dir(&self) -> PathBuf {
            self.base.join("curator")
        }
        fn file(&self, key: &str) -> PathBuf {
            self.dir()
                .join(format!("{}.json", hex_bytes(key.as_bytes())))
        }
    }
    impl CuratorListStore for RealCuratorListStore {
        fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool> {
            let _g = self.lock.lock().expect("curator lock");
            if let Some(cur) = read_json::<CuratorRec>(&self.file(key))? {
                if created_at <= cur.created_at {
                    return Ok(false);
                }
            }
            write_json(
                &self.file(key),
                &CuratorRec {
                    key: key.to_string(),
                    created_at,
                    json: json.to_string(),
                },
            )?;
            Ok(true)
        }
        fn all_lists(&self) -> SysResult<Vec<String>> {
            let rd = match fs::read_dir(self.dir()) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(io_err("read_dir curator", e)),
            };
            let mut recs = vec![];
            for entry in rd {
                let path = entry.map_err(|e| io_err("dir entry", e))?.path();
                if !is_json(&path) {
                    continue;
                }
                if let Some(rec) = read_json::<CuratorRec>(&path)? {
                    recs.push(rec);
                }
            }
            recs.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(recs.into_iter().map(|r| r.json).collect())
        }
    }

    // ---- single-blob stores ----------------------------------------------

    macro_rules! blob_store {
        ($name:ident, $trait:ident, $file:literal) => {
            pub struct $name {
                base: PathBuf,
            }
            impl Default for $name {
                fn default() -> Self {
                    Self::with_base(DEFAULT_BASE)
                }
            }
            impl $name {
                pub fn with_base(base: impl Into<PathBuf>) -> Self {
                    Self { base: base.into() }
                }
                fn path(&self) -> PathBuf {
                    self.base.join($file)
                }
            }
            impl $trait for $name {
                fn save_snapshot(&self, json: &str) -> SysResult<()> {
                    atomic_write(&self.path(), json.as_bytes())
                }
                fn load_snapshot(&self) -> SysResult<Option<String>> {
                    read_opt(&self.path())
                }
            }
        };
    }
    blob_store!(RealUsageStore, UsageStore, "usage.json");
    blob_store!(RealExtensionStore, ExtensionStore, "extension.json");

    pub struct RealPairingStore {
        base: PathBuf,
    }
    impl Default for RealPairingStore {
        fn default() -> Self {
            Self::with_base(DEFAULT_BASE)
        }
    }
    impl RealPairingStore {
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self { base: base.into() }
        }
        fn path(&self) -> PathBuf {
            self.base.join("pairing.json")
        }
    }
    impl PairingStore for RealPairingStore {
        fn save(&self, json: &str) -> SysResult<()> {
            atomic_write(&self.path(), json.as_bytes())
        }
        fn load(&self) -> SysResult<Option<String>> {
            read_opt(&self.path())
        }
        fn clear(&self) -> SysResult<()> {
            match fs::remove_file(self.path()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove pairing", e)),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn tmp(name: &str) -> PathBuf {
            let p =
                std::env::temp_dir().join(format!("charter-real-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            p
        }
        fn rid(b: u8) -> ReqId {
            ReqId::from_bytes([b; 32])
        }

        #[test]
        fn consumed_id_replay_purge_and_durable() {
            let base = tmp("consumed");
            {
                let s = RealConsumedIdStore::with_base(&base);
                assert!(s.check_and_consume(&rid(1), 9999).unwrap());
                assert!(!s.check_and_consume(&rid(1), 9999).unwrap()); // replay
                assert!(s.check_and_consume(&rid(2), 9999).unwrap());
            } // dropped -> simulate restart
            let s2 = RealConsumedIdStore::with_base(&base);
            assert!(!s2.check_and_consume(&rid(1), 9999).unwrap()); // still remembered
            s2.purge_expired(10_000).unwrap(); // exp 9999 <= now -> dropped
            assert!(s2.check_and_consume(&rid(1), 20_000).unwrap()); // consumable again
        }

        #[test]
        fn clause_store_rollback() {
            let s = RealClauseStore::with_base(tmp("clause"));
            assert!(s.put_clause(31113, 100, r#"{"a":1}"#).unwrap());
            assert!(!s.put_clause(31113, 100, r#"{"a":2}"#).unwrap()); // equal
            assert!(!s.put_clause(31113, 50, r#"{"a":3}"#).unwrap()); // lower
            assert!(s.put_clause(31113, 200, r#"{"a":4}"#).unwrap()); // higher
            assert_eq!(s.highest_issued_at(31113).unwrap(), Some(200));
            assert_eq!(s.get_clause(31113).unwrap(), Some(r#"{"a":4}"#.to_string()));
        }

        #[test]
        fn child_clause_rollback_per_subject_kind_and_durable() {
            let base = tmp("childclause");
            {
                let s = RealChildClauseStore::with_base(&base);
                // Write the HIGHER kind first (2.json before 1.json) so the
                // ascending-kind `clauses_for` result below is load-bearing on
                // the explicit sort, not on filesystem enumeration order.
                assert!(s.put_child_clause("aa", 2, 100, r#"{"b":1}"#).unwrap());
                assert!(s.put_child_clause("aa", 1, 100, r#"{"s":1}"#).unwrap());
                assert!(!s.put_child_clause("aa", 1, 100, r#"{"s":2}"#).unwrap()); // equal
                assert!(!s.put_child_clause("aa", 1, 50, r#"{"s":3}"#).unwrap()); // lower
                assert!(s.put_child_clause("aa", 1, 200, r#"{"s":4}"#).unwrap()); // higher
                assert!(s.put_child_clause("bb", 1, 100, r#"{"s":"bob"}"#).unwrap());
            }
            // reopen -> durable + still rollback-protected
            let s2 = RealChildClauseStore::with_base(&base);
            assert_eq!(s2.highest_issued_at("aa", 1).unwrap(), Some(200));
            assert!(!s2.put_child_clause("aa", 1, 200, r#"{"s":9}"#).unwrap());
            assert_eq!(
                s2.clauses_for("aa").unwrap(),
                vec![(1, r#"{"s":4}"#.to_string()), (2, r#"{"b":1}"#.to_string())]
            );
            assert_eq!(
                s2.clauses_for("missing").unwrap(),
                Vec::<(u16, String)>::new()
            );
        }

        #[test]
        fn child_clause_subject_cannot_escape_base_dir() {
            // The `safe()` sanitiser must keep a path-separator / dot-dot-laden
            // subject from writing outside <base>/children/. A crafted subject is
            // filtered down to its hex chars, so it can neither traverse out nor
            // collide a sibling away.
            let base = tmp("childclause-traversal");
            let s = RealChildClauseStore::with_base(&base);
            // `../` and non-hex chars are stripped; only hex survives.
            assert!(s
                .put_child_clause("../../etc/aa", 1, 100, r#"{"s":"evil"}"#)
                .unwrap());
            // The on-disk file resolves UNDER the base, never above it.
            let dir = std::fs::read_dir(base.join("children")).unwrap();
            for entry in dir.flatten() {
                let canon = entry.path().canonicalize().unwrap();
                assert!(
                    canon.starts_with(base.canonicalize().unwrap()),
                    "clause file {canon:?} escaped the base dir"
                );
            }
            // It round-trips via the SANITISED key ("ecaa" — the surviving hex
            // of "../../etc/aa": '.' '/' and the non-hex 't' are all stripped).
            assert_eq!(
                s.get_child_clause("ecaa", 1).unwrap(),
                Some(r#"{"s":"evil"}"#.to_string())
            );
            // Case folds: "AA" and "aa" address the same slot.
            assert!(s.put_child_clause("AA", 1, 100, r#"{"s":"up"}"#).unwrap());
            assert_eq!(
                s.get_child_clause("aa", 1).unwrap(),
                Some(r#"{"s":"up"}"#.to_string())
            );
        }

        #[test]
        fn curator_rollback_listing_durable() {
            let base = tmp("curator");
            {
                let s = RealCuratorListStore::with_base(&base);
                assert!(s.put_list("aa:main", 10, r#"{"c":"aa"}"#).unwrap());
                assert!(!s.put_list("aa:main", 10, r#"{"c":"x"}"#).unwrap());
                assert!(!s.put_list("aa:main", 5, r#"{"c":"y"}"#).unwrap());
                assert!(s.put_list("aa:main", 20, r#"{"c":"aaN"}"#).unwrap());
                assert!(s.put_list("bb:main", 1, r#"{"c":"bb"}"#).unwrap());
            }
            let s2 = RealCuratorListStore::with_base(&base);
            assert_eq!(
                s2.all_lists().unwrap(),
                vec![r#"{"c":"aaN"}"#.to_string(), r#"{"c":"bb"}"#.to_string()]
            );
            assert!(!s2.put_list("aa:main", 20, r#"{"c":"z"}"#).unwrap()); // still protected
        }

        #[test]
        fn pending_roundtrip() {
            let s = RealPendingStore::with_base(tmp("pending"));
            assert_eq!(s.list().unwrap().len(), 0);
            s.put("r1", r#"{"x":1}"#).unwrap();
            s.put("r2", r#"{"x":2}"#).unwrap();
            let l = s.list().unwrap();
            assert_eq!(l.len(), 2);
            assert!(l.iter().any(|(id, j)| id == "r1" && j == r#"{"x":1}"#));
            s.remove("r1").unwrap();
            assert_eq!(s.list().unwrap().len(), 1);
            s.remove("absent").unwrap(); // no-op
        }

        #[test]
        fn blob_and_pairing_roundtrip() {
            let base = tmp("blob");
            let u = RealUsageStore::with_base(&base);
            assert_eq!(u.load_snapshot().unwrap(), None);
            u.save_snapshot(r#"{"u":1}"#).unwrap();
            assert_eq!(u.load_snapshot().unwrap(), Some(r#"{"u":1}"#.to_string()));
            let p = RealPairingStore::with_base(&base);
            p.save(r#"{"pair":true}"#).unwrap();
            assert_eq!(p.load().unwrap(), Some(r#"{"pair":true}"#.to_string()));
        }
    }
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
pub use real::{
    RealChildClauseStore, RealClauseStore, RealConsumedIdStore, RealCuratorListStore,
    RealExtensionStore, RealPairingStore, RealPendingStore, RealUsageStore,
};

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use charter_primitives::ReqId;

    fn rid(b: u8) -> ReqId {
        ReqId::from_bytes([b; 32])
    }

    #[test]
    fn mock_consumed_id_rejects_replay() {
        let disk = MockDisk::new();
        let store = MockConsumedIdStore::new(disk);
        assert!(store.check_and_consume(&rid(1), 9999).unwrap());
        assert!(!store.check_and_consume(&rid(1), 9999).unwrap()); // replay
        assert!(store.check_and_consume(&rid(2), 9999).unwrap());
    }

    #[test]
    fn mock_consumed_id_durable_across_reopen() {
        let disk = MockDisk::new();
        {
            let a = MockConsumedIdStore::new(disk.clone());
            assert!(a.check_and_consume(&rid(7), 9999).unwrap());
        } // store A dropped — simulates a restart
        let b = MockConsumedIdStore::new(disk);
        assert!(!b.check_and_consume(&rid(7), 9999).unwrap()); // still rejected
    }

    #[test]
    fn mock_clause_store_rejects_lower_issued_at() {
        let disk = MockDisk::new();
        let store = MockClauseStore::new(disk);
        assert!(store.put_clause(31113, 100, "{\"a\":1}").unwrap());
        assert!(!store.put_clause(31113, 100, "{\"a\":2}").unwrap()); // equal -> rollback
        assert!(!store.put_clause(31113, 50, "{\"a\":3}").unwrap()); // lower -> rollback
        assert!(store.put_clause(31113, 200, "{\"a\":4}").unwrap()); // higher -> ok
        assert_eq!(store.highest_issued_at(31113).unwrap(), Some(200));
        assert_eq!(
            store.get_clause(31113).unwrap(),
            Some("{\"a\":4}".to_string())
        );
    }

    #[test]
    fn curator_list_store_rollback_and_listing() {
        let disk = MockDisk::new();
        let s = MockCuratorListStore::new(disk);
        assert!(s.put_list("aa:main", 10, r#"{"curator":"aa"}"#).unwrap());
        assert!(!s.put_list("aa:main", 10, r#"{"curator":"aa2"}"#).unwrap()); // equal -> rollback
        assert!(!s.put_list("aa:main", 5, r#"{"curator":"aa3"}"#).unwrap()); // lower -> rollback
        assert!(s.put_list("aa:main", 20, r#"{"curator":"aaN"}"#).unwrap()); // higher -> ok
        assert!(s.put_list("bb:main", 1, r#"{"curator":"bb"}"#).unwrap());
        let all = s.all_lists().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&r#"{"curator":"aaN"}"#.to_string()));
        assert!(all.contains(&r#"{"curator":"bb"}"#.to_string()));
    }

    #[test]
    fn curator_list_store_durable_across_reopen() {
        let disk = MockDisk::new();
        {
            let a = MockCuratorListStore::new(disk.clone());
            assert!(a.put_list("aa:main", 7, r#"{"curator":"aa"}"#).unwrap());
        }
        let b = MockCuratorListStore::new(disk);
        assert_eq!(b.all_lists().unwrap().len(), 1);
        assert!(!b.put_list("aa:main", 7, r#"{"curator":"x"}"#).unwrap()); // still rollback-protected
    }

    #[test]
    fn pending_store_roundtrip() {
        let disk = MockDisk::new();
        let s = MockPendingStore::new(disk);
        s.put("r1", "{}").unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        s.remove("r1").unwrap();
        assert_eq!(s.list().unwrap().len(), 0);
    }

    const ALICE: &str = "aa";
    const BOB: &str = "bb";
    const SCHEDULE: u16 = 1;
    const BUDGET: u16 = 2;

    #[test]
    fn child_clause_store_rollback_per_subject_and_kind() {
        let disk = MockDisk::new();
        let s = MockChildClauseStore::new(disk);
        // Alice/schedule rolls back independently of Alice/budget and Bob.
        assert!(s
            .put_child_clause(ALICE, SCHEDULE, 100, r#"{"s":1}"#)
            .unwrap());
        assert!(!s
            .put_child_clause(ALICE, SCHEDULE, 100, r#"{"s":2}"#)
            .unwrap()); // equal -> rollback
        assert!(!s
            .put_child_clause(ALICE, SCHEDULE, 50, r#"{"s":3}"#)
            .unwrap()); // lower -> rollback
        assert!(s
            .put_child_clause(ALICE, SCHEDULE, 200, r#"{"s":4}"#)
            .unwrap()); // higher -> ok
                        // Same issued_at on a DIFFERENT (subject, kind) is accepted — independent slots.
        assert!(s
            .put_child_clause(ALICE, BUDGET, 100, r#"{"b":1}"#)
            .unwrap());
        assert!(s
            .put_child_clause(BOB, SCHEDULE, 100, r#"{"s":"bob"}"#)
            .unwrap());
        assert_eq!(s.highest_issued_at(ALICE, SCHEDULE).unwrap(), Some(200));
        assert_eq!(
            s.get_child_clause(ALICE, SCHEDULE).unwrap(),
            Some(r#"{"s":4}"#.to_string())
        );
    }

    #[test]
    fn child_clauses_for_lists_one_subject_in_kind_order() {
        let disk = MockDisk::new();
        let s = MockChildClauseStore::new(disk);
        s.put_child_clause(ALICE, BUDGET, 10, r#"{"b":1}"#).unwrap();
        s.put_child_clause(ALICE, SCHEDULE, 10, r#"{"s":1}"#)
            .unwrap();
        s.put_child_clause(BOB, SCHEDULE, 10, r#"{"s":"bob"}"#)
            .unwrap();
        // Alice's two clauses, ascending kind; Bob's excluded.
        let got = s.clauses_for(ALICE).unwrap();
        assert_eq!(
            got,
            vec![
                (SCHEDULE, r#"{"s":1}"#.to_string()),
                (BUDGET, r#"{"b":1}"#.to_string())
            ]
        );
        assert_eq!(s.clauses_for("unknown").unwrap(), vec![]);
    }

    #[test]
    fn child_clause_durable_across_reopen() {
        let disk = MockDisk::new();
        {
            let a = MockChildClauseStore::new(disk.clone());
            assert!(a
                .put_child_clause(ALICE, SCHEDULE, 7, r#"{"s":1}"#)
                .unwrap());
        }
        let b = MockChildClauseStore::new(disk);
        assert_eq!(b.clauses_for(ALICE).unwrap().len(), 1);
        assert!(!b
            .put_child_clause(ALICE, SCHEDULE, 7, r#"{"s":2}"#)
            .unwrap()); // still protected
    }
}
