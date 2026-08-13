//! The durable outbound spool (port-spec §2.2): when EVERY relay refuses a
//! publish, the already-built wrap is parked on disk and retried on each slow
//! poll — a phone's offline windows are routine, not exceptional, and Linux's
//! fire-and-forget at runtime.rs:139-141 is not acceptable here. STATUS spools
//! LATEST-ONLY per subject (a stale heartbeat is worse than none); REQUESTs
//! queue in order. Entries older than 7 days are swept (the guardian-side
//! wrap-jitter window would reject them anyway).

use std::path::{Path, PathBuf};

use charter_primitives::NostrEvent;

/// Spooled wraps older than this are dropped (matches the 2-day inbound
/// jitter bound with margin — after a week nobody wants the retry).
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    at: u64,
    wrap: NostrEvent,
}

pub struct Outbox {
    dir: PathBuf,
}

impl Outbox {
    pub fn new(base: &Path) -> Outbox {
        let dir = base.join("outbox");
        let _ = std::fs::create_dir_all(&dir);
        Outbox { dir }
    }

    /// Park a STATUS wrap — latest-only per subject (overwrite).
    pub fn put_status(&self, subject_hex: &str, wrap: &NostrEvent, now: u64) {
        // The subject is already lowercase hex (validated upstream); guard
        // anyway so a hostile value can never traverse paths.
        if !subject_hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return;
        }
        self.write(&format!("status-{subject_hex}.json"), wrap, now);
    }

    /// Park a REQUEST wrap — ordered queue.
    pub fn put_request(&self, wrap: &NostrEvent, now: u64) {
        self.write(&format!("req-{now}-{}.json", wrap.id.to_hex()), wrap, now);
    }

    fn write(&self, name: &str, wrap: &NostrEvent, now: u64) {
        if let Ok(json) = serde_json::to_string(&Entry {
            at: now,
            wrap: wrap.clone(),
        }) {
            let _ = std::fs::write(self.dir.join(name), json);
        }
    }

    /// Every parked wrap, oldest-first, expired entries swept as we pass.
    pub fn entries(&self, now: u64) -> Vec<(PathBuf, NostrEvent)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(PathBuf, u64, NostrEvent)> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match serde_json::from_str::<Entry>(&text) {
                Ok(entry) if now.saturating_sub(entry.at) <= MAX_AGE_SECS => {
                    out.push((path, entry.at, entry.wrap));
                }
                _ => {
                    // Expired or corrupt — sweep.
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        out.sort_by_key(|(_, at, _)| *at);
        out.into_iter().map(|(p, _, w)| (p, w)).collect()
    }

    pub fn remove(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    pub fn len(&self) -> usize {
        std::fs::read_dir(&self.dir).map(|r| r.count()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
