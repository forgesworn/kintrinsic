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
    /// For a parked REQUEST: what it asks, so a wrap whose recipient has
    /// since left the signer set can be built afresh for the current ones
    /// (a wrap is sealed to one key and is never re-addressed). Absent for
    /// STATUS and for entries parked before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request: Option<ParkedRequest>,
}

/// The REQUEST behind a parked wrap.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ParkedRequest {
    /// The request id, hex: the key into the broker's request records.
    pub req_id: String,
    /// The request payload JSON the wrap was built from.
    pub json: String,
}

/// One parked wrap and what the spool knows about it.
pub struct Parked {
    pub path: PathBuf,
    pub wrap: NostrEvent,
    /// When it was first parked (the 7-day sweep counts from here).
    pub at: u64,
    pub request: Option<ParkedRequest>,
}

/// The `reqId` of a request payload JSON.
fn request_id(request_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(request_json)
        .ok()?
        .get("reqId")?
        .as_str()
        .map(String::from)
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

    /// Park a STATUS wrap — latest-only per (subject, recipient) when the
    /// ward sends STATUS to more than one key (rule 9), so one recipient's
    /// parked STATUS never replaces another's. `None` is [`Self::put_status`].
    pub fn put_status_for(
        &self,
        subject_hex: &str,
        recipient_hex: Option<&str>,
        wrap: &NostrEvent,
        now: u64,
    ) {
        let Some(recipient) = recipient_hex else {
            return self.put_status(subject_hex, wrap, now);
        };
        if !subject_hex.chars().all(|c| c.is_ascii_hexdigit())
            || !recipient.chars().all(|c| c.is_ascii_hexdigit())
        {
            return;
        }
        self.write(&format!("status-{subject_hex}-{recipient}.json"), wrap, now);
    }

    /// Park a REQUEST wrap — ordered queue. `request_json` is the payload the
    /// wrap was built from; a wrap whose recipient has left the signer set
    /// is rebuilt from it. `at` is when the ask was first parked: a rebuilt
    /// wrap keeps the original date, so the 7-day sweep still counts from
    /// the ask.
    pub fn put_request(&self, wrap: &NostrEvent, request_json: &str, at: u64) {
        let request = request_id(request_json).map(|req_id| ParkedRequest {
            req_id,
            json: request_json.to_string(),
        });
        self.write_entry(
            &format!("req-{at}-{}.json", wrap.id.to_hex()),
            wrap,
            at,
            request,
        );
    }

    fn write(&self, name: &str, wrap: &NostrEvent, now: u64) {
        self.write_entry(name, wrap, now, None);
    }

    fn write_entry(&self, name: &str, wrap: &NostrEvent, at: u64, request: Option<ParkedRequest>) {
        if let Ok(json) = serde_json::to_string(&Entry {
            at,
            wrap: wrap.clone(),
            request,
        }) {
            let _ = std::fs::write(self.dir.join(name), json);
        }
    }

    /// Every parked wrap, oldest-first, expired entries swept as we pass.
    pub fn entries(&self, now: u64) -> Vec<Parked> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Parked> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match serde_json::from_str::<Entry>(&text) {
                Ok(entry) if now.saturating_sub(entry.at) <= MAX_AGE_SECS => {
                    out.push(Parked {
                        path,
                        wrap: entry.wrap,
                        at: entry.at,
                        request: entry.request,
                    });
                }
                _ => {
                    // Expired or corrupt — sweep.
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        out.sort_by_key(|p| p.at);
        out
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
