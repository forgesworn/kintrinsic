//! `exec.allow` (Flow B): admit re-hashes the COPIED bytes (TOCTOU close),
//! stores under the sha256 filename, validates + escapes the display name,
//! trust-failure is fail-closed, and the full mocked loop approves a binary by
//! hash — never a path.

#![cfg(feature = "mock")]

use std::sync::Arc;

use charter_primitives::{Nonce, PubKey, ReqId, Sha256Hex};
use charter_proto::OpType;
use charter_sys::effects::{ApprovedExecStore, MockApprovedExecStore, MockTrustDb};
use charter_sys::persistence::{MockConsumedIdStore, MockDisk};
use charter_sys::MockSystem;
use charter_transport::ScriptedEntropy;
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charter_verify::{verify_grant, VerifiedGrant, VerifyParams};
use charterd::enactor::{EnactContext, Enactor};
use charterd::enactors::exec_allow::{plan_exec_allow, ExecAllowEnactor, ExecRequestError};
use charterd::ports::NullEventSink;
use charterd::{Broker, EnactorRegistry, ExecPathError, FileKind, ProbeExec, RequestState};

const NOW: u64 = 1_700_001_000;

fn rid() -> ReqId {
    ReqId::from_bytes([0xA1; 32])
}
fn non() -> Nonce {
    Nonce::from_bytes([0xB2; 32])
}
fn sha_hex(b: &[u8]) -> String {
    Sha256Hex::from_bytes(charter_crypto::sha256(b)).to_hex()
}

fn exec_grant(name: &str, sha_hex: &str, size: u64) -> VerifiedGrant {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .op(OpType::ExecAllow)
        .params(serde_json::json!({"name": name, "sha256": sha_hex, "size": size}))
        .build(&g);
    let pk = g.pubkey();
    let store = MockConsumedIdStore::new(MockDisk::new());
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::ExecAllow,
        now: NOW,
    };
    verify_grant(&ev, &p, &store).unwrap()
}

fn ctx(path: &str) -> EnactContext {
    EnactContext {
        source_path: Some(path.to_string()),
        now_unix: Some(NOW as i64),
        eod_unix: None,
    }
}

/// A real file on disk, plus the mock store's in-memory copy of its bytes.
///
/// The enactor now RE-OPENS the bound path before it admits anything
/// (`openat2`, `O_NOFOLLOW | O_NONBLOCK`, `fstat`-regular-file), so a made-up
/// path string is no longer enactable — which is the whole point of the fix.
/// The mock store still answers from its own map, so the bytes it hashes and
/// the bytes on disk can be made to disagree where a test wants that.
fn real_source(store: &MockApprovedExecStore, name: &str, bytes: &[u8]) -> String {
    let dir = tree(name);
    let p = dir.join("app.bin");
    std::fs::write(&p, bytes).unwrap();
    let s = p.to_str().expect("utf-8 temp path").to_string();
    store.put_source(&s, bytes);
    s
}

/// A throwaway directory for one test, removed and recreated on entry so a
/// crashed previous run cannot leak into this one.
fn tree(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("charterd-exec-flow-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn admit_hashes_copied_bytes_not_source_reread() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "tamper", b"v1");
    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let grant = exec_grant("Game", &sha_hex(b"v1"), 2);
    // The source is swapped between inspect and admit — the copied bytes now
    // hash differently, so admit refuses and stores NOTHING.
    store.mutate_source(&path, b"v2-tampered");
    let err = enactor.enact(&grant, &ctx(&path)).await.unwrap_err();
    assert!(err.is_terminal());
    assert!(store.list().await.unwrap().is_empty());
    assert!(trust.trusted().is_empty());
}

#[tokio::test]
async fn admit_stores_under_sha_filename_and_escapes_name() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "sha-name", b"the binary");
    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let sha = sha_hex(b"the binary");
    let grant = exec_grant("Super Game", &sha, 10);
    enactor.enact(&grant, &ctx(&path)).await.unwrap();

    assert_eq!(store.list().await.unwrap(), vec![sha.clone()]);
    assert_eq!(trust.trusted(), vec![sha.clone()]);
    let launchers = store.launchers();
    assert_eq!(launchers.len(), 1);
    let (lsha, lname, desktop) = &launchers[0];
    assert_eq!(lsha, &sha);
    assert_eq!(lname, "Super Game");
    // Exec points at the store sha path, NOT the display name.
    assert!(desktop.contains(&format!("/var/lib/charter/approved/{sha}/run")));
    assert!(!desktop.contains("app.bin"));
}

#[tokio::test]
async fn name_traversal_rejected() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "badname", b"x");
    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let grant = exec_grant("../../etc/cron.d/evil", &sha_hex(b"x"), 1);
    let err = enactor.enact(&grant, &ctx(&path)).await.unwrap_err();
    assert!(err.is_terminal());
    assert!(
        store.list().await.unwrap().is_empty(),
        "nothing stored on bad name"
    );
}

#[tokio::test]
async fn trust_failure_leaves_binary_untrusted() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "untrusted", b"x");
    let trust = Arc::new(MockTrustDb::new());
    trust.set_fail(true);
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let grant = exec_grant("Game", &sha_hex(b"x"), 1);
    let err = enactor.enact(&grant, &ctx(&path)).await.unwrap_err();
    // Fail-closed: stored but UNTRUSTED (cannot execute); transient (retryable).
    assert!(matches!(err, charterd::error::EnactError::Transient(_)));
    assert_eq!(store.list().await.unwrap().len(), 1);
    assert!(trust.trusted().is_empty());
}

/// THE BLOCKER. A FIFO where the bound candidate was is the cheapest way a
/// ward has of wedging the enforcer: `fs::metadata` follows the name and calls
/// it zero bytes, and the `File::open` that used to follow blocks until a
/// writer appears — inside a synchronous admit, on the tick. `WatchdogSec`
/// then kills charterd and `ExecStopPost --thaw-all` unfreezes every managed
/// child, so "mkfifo" is "enforcement off". The enact must COME BACK, and
/// come back refusing.
///
/// The timeout is the assertion: without the fix this test does not fail, it
/// hangs.
#[tokio::test]
async fn a_fifo_at_the_bound_path_fails_the_enact_instead_of_wedging_the_tick() {
    let store = Arc::new(MockApprovedExecStore::new());
    let dir = tree("fifo");
    let path = dir.join("app.bin");
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: plain `mkfifo(3)` on a path in this test's own throwaway dir.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    let path = path.to_str().unwrap().to_string();
    // The store would happily hand over bytes for it; the enactor never gets
    // that far, because the file it re-opens is not a regular file.
    store.put_source(&path, b"never read");

    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let grant = exec_grant("Game", &sha_hex(b"never read"), 10);
    let err = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        enactor.enact(&grant, &ctx(&path)),
    )
    .await
    .expect("the enact must return inside the tick, not block on the FIFO")
    .unwrap_err();

    assert!(err.is_terminal(), "{err:?}");
    assert!(store.list().await.unwrap().is_empty());
    assert!(trust.trusted().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A symlink swapped in at the bound path between the request and the grant —
/// the TOCTOU the enact-time re-open closes. It points at a perfectly ordinary
/// readable file, and it is still refused: the bound path is re-opened under
/// `RESOLVE_NO_SYMLINKS`, so no component of it can be re-pointed after the
/// guard walked it.
#[tokio::test]
async fn a_symlink_swapped_in_after_submit_is_refused_at_enact() {
    let store = Arc::new(MockApprovedExecStore::new());
    let dir = tree("swap");
    let real = dir.join("elsewhere.bin");
    std::fs::write(&real, b"other bytes").unwrap();
    let bound = dir.join("app.bin");
    std::os::unix::fs::symlink(&real, &bound).unwrap();
    let path = bound.to_str().unwrap().to_string();
    store.put_source(&path, b"other bytes");

    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store.clone(), trust.clone());
    let grant = exec_grant("Game", &sha_hex(b"other bytes"), 11);
    let err = enactor.enact(&grant, &ctx(&path)).await.unwrap_err();

    assert!(err.is_terminal(), "{err:?}");
    assert!(
        store.list().await.unwrap().is_empty(),
        "nothing is admitted through a swapped symlink"
    );
    assert!(trust.trusted().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn missing_source_path_binding_is_terminal() {
    let store = Arc::new(MockApprovedExecStore::new());
    let trust = Arc::new(MockTrustDb::new());
    let enactor = ExecAllowEnactor::new(store, trust);
    let grant = exec_grant("Game", &sha_hex(b"x"), 1);
    let err = enactor
        .enact(&grant, &EnactContext::default())
        .await
        .unwrap_err();
    assert!(err.is_terminal());
}

// --- Request-building seam (the D-Bus `submit_request` exec.allow path) -------
//
// These drive `plan_exec_allow` — the exact function the live `submit_request`
// calls to turn a `charter run <path>` request into the (params, source_path) it
// hands the broker. A full D-Bus round-trip needs a real bus + a real broker over
// a real relay, so this exercises the smallest real seam instead: the one that
// would have caught the shipped bug where the daemon bound `None` for every op.

/// A fake privileged probe: the real one drops to the child's uid via `runuser`;
/// here the test picks the file kind + readability directly.
struct FakeProbe {
    kind: FileKind,
    readable: bool,
}
impl ProbeExec for FakeProbe {
    fn can_read_as_uid(&self, _path: &str, _uid: u32) -> bool {
        self.readable
    }
    fn file_kind(&self, _path: &str) -> FileKind {
        self.kind
    }
}

#[tokio::test]
async fn submit_binds_the_real_source_path_not_none() {
    // REGRESSION GUARD: the live `submit_request` used to publish the client's
    // raw `{"path": ...}` with `source_path = None` for EVERY op, so exec.allow
    // always failed to enact with "no source path bound to reqId". The daemon
    // must instead run the confused-deputy guard, inspect the bytes server-side,
    // and bind the REAL path. If someone reverts the seam to drop the path, the
    // `source_path` assertion below fails.
    // 03-B8: the guard now OPENS the candidate (openat2, RESOLVE_BENEATH) and
    // hands the descriptor to `inspect_from_file`, so the candidate and its
    // managed root have to be real files on a real filesystem — a made-up path
    // string is no longer admissible, which is the point of the fix.
    let tree = tree("plan");
    let managed_root = tree.join("ward");
    std::fs::create_dir_all(&managed_root).unwrap();
    let real_path = managed_root.join("game.AppImage");
    std::fs::write(&real_path, b"appimage bytes").unwrap(); // 14 bytes
    let path = real_path.to_str().unwrap();

    let store = MockApprovedExecStore::new();
    store.put_source(path, b"appimage bytes"); // 14 bytes
    let probe = FakeProbe {
        kind: FileKind::Regular,
        readable: true,
    };

    let client_params = serde_json::json!({ "path": path });
    let (params, source_path) = plan_exec_allow(
        &client_params,
        1000,
        &store,
        managed_root.to_str().unwrap(),
        &probe,
    )
    .await
    .expect("a readable in-tree regular file passes the guard + inspect");

    // The bound source path is the REAL path — this is exactly the None bug —
    // and specifically the guard's RESOLVED path, not the raw string the ward
    // sent, so the enactor re-opens something charterd itself walked.
    assert_eq!(
        source_path,
        std::fs::canonicalize(&real_path).unwrap().to_str().unwrap(),
        "source_path must be bound (resolved), not None/empty"
    );
    // The published params are the SERVER-inspected ExecAllowParams (name +
    // sha256 + size) — never the client's raw {"path": ...}.
    assert!(
        params.get("path").is_none(),
        "the raw client path must not be published as request params"
    );
    assert_eq!(params["name"], "game");
    assert_eq!(params["size"], 14);
    assert_eq!(params["sha256"], sha_hex(b"appimage bytes"));
    let _ = std::fs::remove_dir_all(&tree);
}

#[tokio::test]
async fn submit_refuses_a_confused_deputy_path() {
    // A path outside the caller's managed tree is rejected BEFORE any request is
    // built — even though charterd (root) could read it. Proves the guard is
    // actually invoked from the live seam (it never was before this fix).
    let store = MockApprovedExecStore::new();
    store.put_source("/etc/shadow", b"root secret");
    let probe = FakeProbe {
        kind: FileKind::Regular,
        readable: true,
    };

    let params = serde_json::json!({ "path": "/etc/shadow" });
    let err = plan_exec_allow(&params, 1000, &store, "/home/managed", &probe)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ExecRequestError::ConfusedDeputy(ExecPathError::OutsideManagedTree)
    );
}

#[tokio::test]
async fn submit_requires_a_path_param() {
    // A malformed request body (no `path`) is a clean error — not a panic and
    // not a published-but-broken request.
    let store = MockApprovedExecStore::new();
    let probe = FakeProbe {
        kind: FileKind::Regular,
        readable: true,
    };
    let err = plan_exec_allow(
        &serde_json::json!({ "nope": 1 }),
        1000,
        &store,
        "/home/managed",
        &probe,
    )
    .await
    .unwrap_err();
    assert_eq!(err, ExecRequestError::MissingPath);
}

// --- Full mocked Flow B loop ----------------------------------------------

type ExecBroker = Broker<MockSystem, charterd::MockTransport, ScriptedEntropy>;

fn build_loop(
    store: Arc<MockApprovedExecStore>,
    trust: Arc<MockTrustDb>,
) -> (ExecBroker, TestGuardian) {
    let sys = MockSystem::new(NOW);
    let guardian = TestGuardian::new();
    let transport = charterd::MockTransport::new(guardian.pubkey(), PubKey::from_bytes([0x42; 32]));
    let mut reg = EnactorRegistry::new();
    reg.register(Box::new(ExecAllowEnactor::new(store, trust)));
    let broker = Broker::new(
        sys,
        transport,
        ScriptedEntropy::new(1),
        reg,
        Box::new(NullEventSink),
        PubKey::from_bytes([0xBB; 32]),
    );
    (broker, TestGuardian::new())
}

fn ids(broker: &ExecBroker, hex: &str) -> (ReqId, Nonce) {
    let rec = broker.status(hex).pop().unwrap();
    (rec.req_id, rec.nonce)
}

#[tokio::test]
async fn flow_b_golden() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "golden", b"appimage bytes");
    let trust = Arc::new(MockTrustDb::new());
    let (broker, guardian) = build_loop(store.clone(), trust.clone());

    let sha = sha_hex(b"appimage bytes");
    let req = broker
        .submit(
            OpType::ExecAllow,
            serde_json::json!({"name": "STK", "sha256": sha, "size": 14}),
            Some(path.clone()),
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .op(OpType::ExecAllow)
        .params(serde_json::json!({"name": "STK", "sha256": sha, "size": 14}))
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;

    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Enacted);
    assert_eq!(store.list().await.unwrap(), vec![sha.clone()]);
    assert_eq!(trust.trusted(), vec![sha]);
}

#[tokio::test]
async fn flow_b_transient_trust_failure_retries_then_enacts() {
    // M8: a transient trust failure must be retried over the SAME in-memory
    // grant (no re-verify) and complete — not left stored-but-untrusted with a
    // burned reqId, and not double-admitted.
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "retry", b"bytes");
    let trust = Arc::new(MockTrustDb::new());
    trust.set_fail_times(1); // the first trust attempt fails transiently
    let (broker, guardian) = build_loop(store.clone(), trust.clone());

    let sha = sha_hex(b"bytes");
    let req = broker
        .submit(
            OpType::ExecAllow,
            serde_json::json!({"name": "STK", "sha256": sha, "size": 5}),
            Some(path.clone()),
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .op(OpType::ExecAllow)
        .params(serde_json::json!({"name": "STK", "sha256": sha, "size": 5}))
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;

    assert_eq!(
        broker.status(&req.to_hex())[0].state,
        RequestState::Enacted,
        "the transient failure is retried and the enact completes"
    );
    assert_eq!(
        store.list().await.unwrap(),
        vec![sha.clone()],
        "admitted exactly once across the retry"
    );
    assert_eq!(trust.trusted(), vec![sha], "trusted on the retry");
}

#[tokio::test]
async fn flow_b_hash_mismatch_refuses() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "mismatch", b"real bytes");
    let trust = Arc::new(MockTrustDb::new());
    let (broker, guardian) = build_loop(store.clone(), trust.clone());

    // The grant authorizes a DIFFERENT hash than the source on disk.
    let wrong_sha = sha_hex(b"some other bytes");
    let req = broker
        .submit(
            OpType::ExecAllow,
            serde_json::json!({"name": "STK", "sha256": wrong_sha, "size": 9}),
            Some(path.clone()),
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .op(OpType::ExecAllow)
        .params(serde_json::json!({"name": "STK", "sha256": wrong_sha, "size": 9}))
        .build(&guardian);
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;

    assert_eq!(broker.status(&req.to_hex())[0].state, RequestState::Failed);
    assert!(store.list().await.unwrap().is_empty());
    assert!(trust.trusted().is_empty());
}

#[tokio::test]
async fn flow_b_replay_no_double_admit() {
    let store = Arc::new(MockApprovedExecStore::new());
    let path = real_source(&store, "replay", b"bytes");
    let trust = Arc::new(MockTrustDb::new());
    let (broker, guardian) = build_loop(store.clone(), trust.clone());

    let sha = sha_hex(b"bytes");
    let req = broker
        .submit(
            OpType::ExecAllow,
            serde_json::json!({"name": "STK", "sha256": sha, "size": 5}),
            Some(path.clone()),
        )
        .await
        .unwrap();
    let (r, n) = ids(&broker, &req.to_hex());
    let grant = GrantBuilder::install_allow(r, n)
        .op(OpType::ExecAllow)
        .params(serde_json::json!({"name": "STK", "sha256": sha, "size": 5}))
        .build(&guardian);
    broker
        .transport()
        .deliver_grant(grant.clone(), guardian.pubkey());
    broker.poll_once().await;
    broker.transport().deliver_grant(grant, guardian.pubkey());
    broker.poll_once().await;
    assert_eq!(
        store.list().await.unwrap().len(),
        1,
        "single-use: admitted once"
    );
}
