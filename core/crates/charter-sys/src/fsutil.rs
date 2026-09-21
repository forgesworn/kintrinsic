//! Shared on-disk helpers for the `real` ports (persistence + file-backed
//! enactors). Every mutating write is **atomic** — temp file in the same dir,
//! fsync, then rename over the target — so a crash mid-write never leaves a
//! half-written file. Reads treat a missing file as `None`, never an error, so
//! a not-yet-provisioned store is just "empty".
//!
//! Only compiled under `--features real`; the headless gate uses the in-memory
//! mocks instead.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{SysError, SysResult};

/// Wrap a low-level error with the operation that produced it.
pub fn io_err<E: std::fmt::Display>(ctx: &str, e: E) -> SysError {
    SysError::Io(format!("{ctx}: {e}"))
}

/// Lowercase hex — derives filesystem-safe filenames from opaque byte keys.
pub fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

/// Atomic write: temp file in the same dir, fsync, then rename over `path`.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> SysResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| SysError::Io("path has no parent".into()))?;
    fs::create_dir_all(dir).map_err(|e| io_err("create_dir_all", e))?;
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| io_err("create tmp", e))?;
        f.write_all(bytes).map_err(|e| io_err("write tmp", e))?;
        f.sync_all().map_err(|e| io_err("fsync tmp", e))?;
    }
    fs::rename(&tmp, path).map_err(|e| io_err("rename", e))
}

/// Read a file to a string, mapping "not found" to `None`.
pub fn read_opt(path: &Path) -> SysResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err("read", e)),
    }
}

/// Read + deserialize a JSON file, mapping "not found" to `None`.
///
/// The three outcomes are deliberately distinct, and a caller walking a
/// directory must keep them so: `Ok(None)` is *no such file*, `Ok(Some)` is a
/// record we have, and `Err` is *the file is there and we cannot read it* —
/// a truncated write, a permissions change, an EIO. Propagating that `Err`
/// out of a walk with `?` abandons every OTHER file in the directory on the
/// strength of one bad one, and callers that then treat the error as "nothing
/// was stored" turn one corrupt record into a wholesale loss of policy. Skip
/// the entry and report it instead.
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> SysResult<Option<T>> {
    match read_opt(path)? {
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| io_err("parse", e)),
        None => Ok(None),
    }
}

/// Serialize + atomically write a value as JSON.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> SysResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| io_err("serialize", e))?;
    atomic_write(path, &bytes)
}

/// `true` if `path` is a `*.json` file (skips in-flight `*.tmp` writes).
pub fn is_json(path: &Path) -> bool {
    path.extension().and_then(|x| x.to_str()) == Some("json")
}
