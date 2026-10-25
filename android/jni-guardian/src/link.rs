//! The carrier's local-link listener (`charter-link`, guardian role), held as
//! one process-global [`GuardianLink`] on a process-global tokio runtime.
//!
//! The JNI wrappers in `lib.rs` only move strings across the boundary; every
//! decision and every JSON shape lives here, in plain functions the host test
//! suite exercises. Shapes are the carrier bridge's (`docs/local-network-
//! design.md`, "Native library entry points" and "Carrier bridge").

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use charter_link::{DeliveryRecord, GuardianEvent, GuardianLink, GuardianLinkConfig, SendState};
use charter_primitives::{EventId, NostrEvent, PubKey};
use serde_json::{json, Value};
use zeroize::Zeroizing;

/// Upper bound on one `next` wait, whatever Kotlin asks for.
const MAX_WAIT_MS: u64 = 60_000;
/// How long a restart waits for the previous listener to release its port
/// (the old accept loop lets go asynchronously).
const REBIND_TRIES: u32 = 20;
const REBIND_STEP: Duration = Duration::from_millis(25);

struct Running {
    link: Arc<GuardianLink>,
    guardian: PubKey,
    dir: PathBuf,
    preferred_port: u16,
}

static LINK: Mutex<Option<Running>> = Mutex::new(None);
static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();

fn global() -> MutexGuard<'static, Option<Running>> {
    LINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The runtime the sockets live on: its own worker thread, created on first
/// start and kept for the life of the process.
fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("kintrinsic-link")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
}

/// The current link, cloned out so no caller holds the global lock while it
/// works (`next` blocks for up to its timeout).
fn current() -> Option<Arc<GuardianLink>> {
    global().as_ref().map(|r| r.link.clone())
}

// ---- JSON shaping (pure) ---------------------------------------------------

pub fn error_json(msg: &str) -> String {
    json!({ "ok": false, "error": msg }).to_string()
}

pub fn ok_json() -> String {
    json!({ "ok": true }).to_string()
}

pub fn start_ok_json(port: u16) -> String {
    json!({ "ok": true, "port": port }).to_string()
}

/// The allowed set: a JSON array of 64-hex machine keys. Entries that are
/// not valid keys are skipped (that only ever denies a machine); a value that
/// is not an array at all is an error, so a garbled push never silently
/// empties the set.
pub fn parse_allowed(json: &str) -> Result<Vec<PubKey>, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "allowed is not JSON".to_string())?;
    let arr = v.as_array().ok_or("allowed is not an array")?;
    let mut out: Vec<PubKey> = arr
        .iter()
        .filter_map(|e| e.as_str().and_then(|s| PubKey::from_hex(s).ok()))
        .collect();
    out.sort_by_key(|k| *k.as_bytes());
    out.dedup();
    Ok(out)
}

/// A JSON array of 64-hex wrap ids; invalid entries are skipped.
pub fn parse_ids(json: &str) -> Vec<EventId> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e.as_str().and_then(|s| EventId::from_hex(s).ok()))
        .collect()
}

pub fn send_json(state: &SendState) -> String {
    serde_json::to_string(state).unwrap_or_else(|_| unavailable_send_json())
}

pub fn unavailable_send_json() -> String {
    json!({ "state": "unavailable" }).to_string()
}

pub fn rejected_json(reason: &str) -> String {
    json!({ "state": "rejected", "reason": reason }).to_string()
}

/// `{"<wrapId>":{"state":…,"at":unix,"machine":hex}}`
pub fn delivery_json(records: &BTreeMap<EventId, DeliveryRecord>) -> String {
    let m: serde_json::Map<String, Value> = records
        .iter()
        .filter_map(|(id, r)| Some((id.to_hex(), serde_json::to_value(r).ok()?)))
        .collect();
    Value::Object(m).to_string()
}

/// `[{"type":"wrap"|"delivery"|"session",…}]`
pub fn events_json(events: &[GuardianEvent]) -> String {
    serde_json::to_string(events).unwrap_or_else(|_| "[]".into())
}

// ---- operations on the global link ----------------------------------------

/// Start the listener, or keep the one already running when it serves the
/// same guardian from the same directory (returns its port). A different
/// guardian key or directory replaces it.
pub fn start(
    state_dir: &str,
    sk: Zeroizing<[u8; 32]>,
    allowed_json: &str,
    preferred_port: i32,
) -> String {
    let Some(guardian) = charter_link::public_key(&sk) else {
        return error_json("invalid guardian key");
    };
    if state_dir.is_empty() {
        return error_json("no state directory");
    }
    let allowed = match parse_allowed(allowed_json) {
        Ok(a) => a,
        Err(e) => return error_json(&e),
    };
    let preferred_port = u16::try_from(preferred_port).unwrap_or(0);
    let dir = PathBuf::from(state_dir);

    let mut g = global();
    if let Some(r) = g.as_ref() {
        if r.guardian == guardian && r.dir == dir {
            r.link.set_allowed(allowed);
            return start_ok_json(r.link.port());
        }
    }
    let previous_port = g.take().map(|r| {
        r.link.shutdown();
        (r.preferred_port, r.link.port())
    });
    if let Some((pref, port)) = previous_port {
        if pref != 0 && pref == preferred_port && port == pref {
            wait_for_port(preferred_port);
        }
    }
    let Some(rt) = runtime() else {
        return error_json("no runtime");
    };
    let cfg = GuardianLinkConfig {
        guardian_sk: sk,
        state_dir: dir.clone(),
        preferred_port,
        allow_loopback: false,
    };
    match GuardianLink::start(cfg, allowed, rt.handle()) {
        Ok(link) => {
            let port = link.port();
            *g = Some(Running {
                link,
                guardian,
                dir,
                preferred_port,
            });
            start_ok_json(port)
        }
        Err(e) => error_json(&format!("could not listen: {e}")),
    }
}

/// Give a just-stopped listener a moment to release `port`, so a restart
/// keeps the advertised port instead of falling back to an ephemeral one.
fn wait_for_port(port: u16) {
    for _ in 0..REBIND_TRIES {
        if std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return;
        }
        std::thread::sleep(REBIND_STEP);
    }
}

pub fn stop() -> String {
    if let Some(r) = global().take() {
        // Explicit: the event thread may still hold a clone, so dropping ours
        // would not end the link.
        r.link.shutdown();
    }
    ok_json()
}

pub fn allow(allowed_json: &str) -> String {
    let allowed = match parse_allowed(allowed_json) {
        Ok(a) => a,
        Err(e) => return error_json(&e),
    };
    match current() {
        Some(link) => {
            link.set_allowed(allowed);
            ok_json()
        }
        None => error_json("link not running"),
    }
}

pub fn send(recipient_hex: &str, wrap_json: &str) -> String {
    let Some(link) = current() else {
        return unavailable_send_json();
    };
    let Ok(recipient) = PubKey::from_hex(recipient_hex) else {
        return rejected_json("bad recipient");
    };
    let Ok(wrap) = serde_json::from_str::<NostrEvent>(wrap_json) else {
        return rejected_json("bad wrap");
    };
    send_json(&link.send(&recipient, wrap))
}

/// Blocks for up to `timeout_ms` (capped). Returns `[]` at once when no link
/// is running; the caller decides whether to wait.
pub fn next(timeout_ms: i32) -> String {
    let Some(link) = current() else {
        return "[]".into();
    };
    let ms = u64::try_from(timeout_ms).unwrap_or(0).min(MAX_WAIT_MS);
    events_json(&link.next_events(Duration::from_millis(ms)))
}

pub fn inbox() -> String {
    current()
        .and_then(|l| serde_json::to_string(&l.inbox()).ok())
        .unwrap_or_else(|| "[]".into())
}

pub fn consume(ids_json: &str) -> String {
    let ids = parse_ids(ids_json);
    match current() {
        Some(link) => {
            if !ids.is_empty() {
                link.consume(&ids);
            }
            ok_json()
        }
        None => error_json("link not running"),
    }
}

pub fn delivery(ids_json: &str) -> String {
    let ids = parse_ids(ids_json);
    match current() {
        Some(link) if !ids.is_empty() => delivery_json(&link.delivery(&ids)),
        _ => "{}".into(),
    }
}

pub fn sessions() -> String {
    current()
        .and_then(|l| serde_json::to_string(&l.sessions()).ok())
        .unwrap_or_else(|| "[]".into())
}

/// Test hook: whether a link is running from `dir`.
#[cfg(test)]
fn running_from(dir: &std::path::Path) -> bool {
    global().as_ref().is_some_and(|r| r.dir == dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_link::{AckResult, DeliveryState, SessionInfo};

    const A: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const B: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

    fn key(n: u8) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([n; 32])
    }

    fn tmp(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "jni-guardian-link-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn allowed_skips_bad_keys_and_dedupes() {
        let got = parse_allowed(&format!(
            r#"["{A}","nothex","{B}","{A}",7,"{}"]"#,
            A.to_uppercase()
        ))
        .unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&PubKey::from_hex(A).unwrap()));
        assert!(got.contains(&PubKey::from_hex(B).unwrap()));
        assert_eq!(parse_allowed("[]").unwrap(), vec![]);
    }

    #[test]
    fn allowed_that_is_not_an_array_is_an_error() {
        assert!(parse_allowed("{}").is_err());
        assert!(parse_allowed("garbage").is_err());
        assert!(parse_allowed(&format!("\"{A}\"")).is_err());
    }

    #[test]
    fn ids_skip_invalid_entries() {
        assert_eq!(parse_ids(&format!(r#"["{A}","x",null]"#)).len(), 1);
        assert!(parse_ids("nope").is_empty());
        assert!(parse_ids("{}").is_empty());
    }

    #[test]
    fn send_states_have_the_bridge_shape() {
        assert_eq!(parse(&send_json(&SendState::Sent)), json!({"state":"sent"}));
        assert_eq!(
            parse(&send_json(&SendState::Queued)),
            json!({"state":"queued"})
        );
        assert_eq!(
            parse(&send_json(&SendState::Rejected("why".into()))),
            json!({"state":"rejected","reason":"why"})
        );
        assert_eq!(
            parse(&unavailable_send_json()),
            json!({"state":"unavailable"})
        );
    }

    #[test]
    fn delivery_is_keyed_by_wrap_id() {
        let id = EventId::from_hex(A).unwrap();
        let m = PubKey::from_hex(B).unwrap();
        let mut recs = BTreeMap::new();
        recs.insert(
            id,
            DeliveryRecord {
                state: DeliveryState::Acked(AckResult::Applied),
                at: 42,
                machine: m,
            },
        );
        assert_eq!(
            parse(&delivery_json(&recs)),
            json!({ A: {"state":"applied","at":42,"machine":B} })
        );
        assert_eq!(delivery_json(&BTreeMap::new()), "{}");
    }

    #[test]
    fn events_and_sessions_have_the_bridge_shape() {
        let m = PubKey::from_hex(B).unwrap();
        let ev = vec![
            GuardianEvent::Session {
                machine: m,
                up: true,
            },
            GuardianEvent::Delivery {
                wrap: EventId::from_hex(A).unwrap(),
                machine: m,
                state: DeliveryState::Received,
            },
        ];
        assert_eq!(
            parse(&events_json(&ev)),
            json!([
                {"type":"session","machine":B,"up":true},
                {"type":"delivery","wrap":A,"machine":B,"state":"received"}
            ])
        );
        let s = serde_json::to_string(&[SessionInfo {
            machine: m,
            since: 9,
        }])
        .unwrap();
        assert_eq!(parse(&s), json!([{"machine":B,"since":9}]));
    }

    /// One test for everything touching the process-global link, so the
    /// parallel test runner never races it.
    #[test]
    fn global_link_lifecycle() {
        // Nothing running: every call degrades, none panics.
        stop();
        assert_eq!(parse(&send(A, "{}")), json!({"state":"unavailable"}));
        assert_eq!(next(10), "[]");
        assert_eq!(inbox(), "[]");
        assert_eq!(sessions(), "[]");
        assert_eq!(delivery(&format!("[\"{A}\"]")), "{}");
        assert_eq!(parse(&allow("[]"))["ok"], json!(false));
        assert_eq!(parse(&consume("[]"))["ok"], json!(false));

        // Bad inputs are refused before anything binds.
        assert_eq!(parse(&start("", key(1), "[]", 0))["ok"], json!(false));
        assert_eq!(parse(&start("/tmp", key(0), "[]", 0))["ok"], json!(false));
        let d1 = tmp("one");
        assert_eq!(
            parse(&start(d1.to_str().unwrap(), key(1), "{}", 0))["ok"],
            json!(false)
        );

        // Start, then the same key and directory again: same link, same port.
        let s1 = parse(&start(
            d1.to_str().unwrap(),
            key(1),
            &format!("[\"{A}\"]"),
            0,
        ));
        assert_eq!(s1["ok"], json!(true), "{s1}");
        let port = s1["port"].as_u64().unwrap();
        assert!(port > 0);
        let first = current().unwrap();
        let s2 = parse(&start(d1.to_str().unwrap(), key(1), "[]", 0));
        assert_eq!(s2["port"].as_u64(), Some(port));
        assert!(Arc::ptr_eq(&first, &current().unwrap()));
        assert_eq!(parse(&allow(&format!("[\"{B}\"]")))["ok"], json!(true));

        // Running: queries answer, a send to an unknown recipient is refused.
        assert_eq!(inbox(), "[]");
        assert_eq!(sessions(), "[]");
        assert_eq!(parse(&send(A, "{}"))["state"], json!("rejected"));
        assert_eq!(parse(&send("zz", "{}"))["state"], json!("rejected"));
        assert_eq!(next(5), "[]");
        assert_eq!(parse(&consume(&format!("[\"{A}\"]")))["ok"], json!(true));

        // A different key restarts it; the old link is shut down.
        let s3 = parse(&start(d1.to_str().unwrap(), key(2), "[]", 0));
        assert_eq!(s3["ok"], json!(true));
        assert!(!Arc::ptr_eq(&first, &current().unwrap()));
        assert!(matches!(
            first.send(&PubKey::from_hex(A).unwrap(), dummy_wrap()),
            SendState::Rejected(_)
        ));

        // A different directory restarts it too.
        let d2 = tmp("two");
        assert_eq!(
            parse(&start(d2.to_str().unwrap(), key(2), "[]", 0))["ok"],
            json!(true)
        );
        assert!(running_from(&d2));

        // Stop: a blocked `next` would return; later calls degrade again.
        assert_eq!(parse(&stop())["ok"], json!(true));
        assert!(current().is_none());
        assert_eq!(parse(&send(A, "{}")), json!({"state":"unavailable"}));
        let _ = std::fs::remove_dir_all(d1);
        let _ = std::fs::remove_dir_all(d2);
    }

    fn dummy_wrap() -> NostrEvent {
        serde_json::from_value(json!({
            "id": A, "pubkey": B, "created_at": 1, "kind": 1059,
            "tags": [["p", A]], "content": "", "sig": "00".repeat(64)
        }))
        .unwrap()
    }
}
