//! Guardian roots and working keys on the Linux warden: the adapter over the
//! real `0600` file store in a state directory (as `charterd` loads it), the
//! broker carrying it, scan-to-pair pinning the ROOT (review 2a, L4), the
//! presence request the daemon serves for `charter-presence`, STATUS to every
//! working key, the RELEASE refusal with two roots, and an existing
//! single-guardian ward migrating in place.

#![cfg(feature = "mock")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use charter_link::AckResult;
use charter_primitives::{kinds, NostrEvent, PubKey};
use charter_proto::ClauseKind;
use charter_spine::ports::NullEventSink;
use charter_spine::{
    Broker, CharterTransportAdapter, EnactorRegistry, GuardianKeys, GuardianStatements,
    LocalIngestOptions,
};
use charter_sys::persistence::{ChildClauseStore, MockDisk, PairingStore};
use charter_sys::relay::{Filter, MockRelayTransport, RelayTransport};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::{CharterTransport, Pairing, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, TestGuardian};
use charterd::guardians::{self, PresenceServed};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const PAIRED_AT: u64 = NOW - 1_000;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);
const ROOT: u8 = 0x11;
const W: u8 = 0x12;
const ROOT2: u8 = 0x13;
const W2: u8 = 0x14;
const STRANGER: u8 = 0x33;
const LOCAL: LocalIngestOptions = LocalIngestOptions {
    handle_release: true,
};

type B = Broker<
    MockSystem,
    CharterTransportAdapter<MockRelayTransport, ScriptedEntropy>,
    ScriptedEntropy,
>;

fn secret(b: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = b;
    s
}

fn pk(seed: u8) -> PubKey {
    PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret(seed)).unwrap())
}

fn machine_pk() -> PubKey {
    pk(0x22)
}

fn pairing_json() -> serde_json::Value {
    json!({
        "guardian_pubkey": pk(ROOT).to_hex(),
        "relays": ["wss://r"],
        "machine": machine_pk().to_hex(),
        "subject_pubkey": SUBJECT.to_hex(),
        "audit_transparency": false,
        "paired_at": PAIRED_AT,
        "relays_updated_at": 0,
    })
}

fn pairing() -> Pairing {
    serde_json::from_value(pairing_json()).unwrap()
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("charterd-gk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A paired ward with its state directory, as `charterd` runs one.
struct Ward {
    dir: PathBuf,
    disk: MockDisk,
    relay: MockRelayTransport,
}

impl Ward {
    fn new(name: &str) -> Self {
        Ward {
            dir: temp_dir(name),
            disk: MockDisk::new(),
            relay: MockRelayTransport::new(),
        }
    }

    /// The daemon's one adapter, loaded from the state directory.
    fn keys(&self) -> GuardianKeys {
        guardians::load_keys(&self.dir, &pairing(), NOW)
    }

    fn broker(&self, keys: &GuardianKeys) -> B {
        let mut t = CharterTransport::new(
            self.relay.clone(),
            ScriptedEntropy::new(7),
            secret(0x22),
            pk(ROOT),
            vec!["wss://r".into()],
        );
        t.set_signer_set(keys.signers());
        let b = Broker::new(
            MockSystem::over_disk(NOW, self.disk.clone()),
            CharterTransportAdapter::new(t),
            ScriptedEntropy::new(1),
            EnactorRegistry::new(),
            Box::new(NullEventSink),
            SUBJECT,
        );
        b.sys().pairing().save(&pairing_json().to_string()).unwrap();
        b.with_guardians(Arc::new(keys.clone()))
    }

    fn sealed(&self, ev: &NostrEvent, sealer: u8, salt: u8) -> NostrEvent {
        let r = WrapRandomness {
            ephemeral_secret: secret(salt | 0x40),
            seal_nonce: [salt; 32],
            wrap_nonce: [salt.wrapping_add(1); 32],
            seal_created_at: NOW,
            wrap_created_at: NOW,
        };
        nip59::wrap(
            &Rumor::from_signed_event(ev),
            &secret(sealer),
            machine_pk().as_bytes(),
            &r,
        )
        .unwrap()
    }
}

impl Drop for Ward {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn clause(seed: u8, issued_at: u64, minutes: u32) -> NostrEvent {
    ClauseBuilder::budget(issued_at)
        .subject(SUBJECT)
        .body(json!({"dailyMinutes": minutes, "tz": "UTC"}))
        .build(&TestGuardian::from_seed(seed))
}

fn statement(
    seed: u8,
    d: String,
    p: PubKey,
    issued_at: u64,
    extra: serde_json::Value,
) -> NostrEvent {
    let mut content = json!({"v": 1, "issuedAt": issued_at});
    for (k, v) in extra.as_object().unwrap() {
        content[k] = v.clone();
    }
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_GUARDIAN_STATEMENT,
        issued_at,
        vec![
            kinds::marker_tag(),
            vec!["d".into(), d],
            vec!["p".into(), p.to_hex()],
        ],
        content.to_string(),
    )
}

fn approval(root: u8, w: u8, issued_at: u64, label: &str) -> NostrEvent {
    statement(
        root,
        format!("kintrinsic/device/{}", pk(w).to_hex()),
        pk(w),
        issued_at,
        json!({"label": label}),
    )
}

fn release(seed: u8) -> NostrEvent {
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_DEVICE_RELEASE,
        NOW,
        vec![kinds::marker_tag()],
        json!({"v": 1, "machine": machine_pk().to_hex(), "issuedAt": NOW}).to_string(),
    )
}

fn held_minutes(b: &B) -> Option<u64> {
    b.sys()
        .child_clauses()
        .get_child_clause(&SUBJECT.to_hex(), ClauseKind::Budget.store_key())
        .unwrap()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["dailyMinutes"].as_u64())
}

async fn ingest(w: &Ward, b: &B, ev: &NostrEvent, sealer: u8, salt: u8) -> AckResult {
    b.ingest_local(&w.sealed(ev, sealer, salt), LOCAL)
        .await
        .result
}

async fn approve(w: &Ward, b: &B, root: u8, working: u8, salt: u8) {
    let r = ingest(
        w,
        b,
        &approval(root, working, NOW - 500, "Mum's phone"),
        working,
        salt,
    )
    .await;
    assert_eq!(r, AckResult::Applied, "approval of {working:#x}");
}

/// R approves W; R names R2 a co-guardian (sealed by W); an administrator
/// asks for a presence code the way `charter-presence` does and the daemon
/// mints it; W2, approved by R2, confirms in person.
async fn add_coguardian(w: &Ward, b: &B, keys: &GuardianKeys) {
    approve(w, b, ROOT, W, 1).await;
    let cg = statement(
        ROOT,
        format!("kintrinsic/guardian/{}", pk(ROOT2).to_hex()),
        pk(ROOT2),
        NOW - 50,
        json!({}),
    );
    assert_eq!(ingest(w, b, &cg, W, 2).await, AckResult::Applied);
    let pending = guardians::build_view(keys, NOW).unwrap().pending;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, "coguardian");
    assert_eq!(pending[0].key, guardians::short_npub(&pk(ROOT2)));
    let d = pending[0].d.clone();

    guardians::request_presence(&w.dir, &d, NOW).unwrap();
    let request = guardians::PresenceRequest {
        d: d.clone(),
        requested_at: NOW,
    };
    assert!(guardians::presence_request_pending(&w.dir));
    let served = guardians::serve_presence_request(&w.dir, keys, NOW, &mut |b| b.fill(0x7e));
    let Some(PresenceServed::Minted(t)) = served else {
        panic!("not minted: {served:?}");
    };
    assert!(!guardians::presence_request_pending(&w.dir));
    let shown = guardians::minted_for(&w.dir, &request, NOW).expect("display copy");
    assert_eq!(shown.token, t.token);
    let mode = std::fs::metadata(w.dir.join(guardians::PRESENCE_TOKEN_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let w2_approval = approval(ROOT2, W2, NOW - 40, "Dad's phone");
    let confirm = sign_event(
        &TestGuardian::from_seed(W2).signer,
        kinds::CHARTER_PRESENCE_CONFIRM,
        NOW,
        vec![kinds::marker_tag()],
        json!({"v": 1, "token": t.token, "statement": cg.id.to_hex(),
               "approval": serde_json::to_value(&w2_approval).unwrap()})
        .to_string(),
    );
    assert_eq!(ingest(w, b, &confirm, W2, 3).await, AckResult::Applied);
    // Spent: the display copy is gone too.
    assert!(guardians::minted_for(&w.dir, &request, NOW).is_none());
}

#[tokio::test]
async fn an_existing_single_guardian_ward_migrates_and_keeps_enforcing() {
    let w = Ward::new("migrate");
    // Paired before the section: pairing.json and a stored clause, no state.
    let keys0 = w.keys();
    let b = w.broker(&keys0);
    b.sys()
        .child_clauses()
        .put_child_clause(
            &SUBJECT.to_hex(),
            ClauseKind::Budget.store_key(),
            NOW - 100,
            &json!({"v": 1, "dailyMinutes": 60, "tz": "UTC"}).to_string(),
        )
        .unwrap();
    // The state was written 0600 by the load.
    let state = w.dir.join(charter_sys::guardian_store::GUARDIAN_STATE_FILE);
    let mode = std::fs::metadata(&state).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // A restart loads it rather than migrating again.
    let keys = w.keys();
    assert_eq!(keys.signer_keys(), vec![pk(ROOT)]);
    let b = w.broker(&keys);
    assert_eq!(
        held_minutes(&b),
        Some(60),
        "the stored rule is still enforced"
    );
    // An older clause from the pinned guardian cannot roll it back…
    assert_ne!(
        ingest(&w, &b, &clause(ROOT, NOW - 200, 600), ROOT, 1).await,
        AckResult::Applied
    );
    assert_eq!(held_minutes(&b), Some(60));
    // …a stranger is refused…
    assert_eq!(
        ingest(&w, &b, &clause(STRANGER, NOW - 10, 600), STRANGER, 2).await,
        AckResult::Refused
    );
    // …and a newer one from the guardian replaces it, as before.
    assert_eq!(
        ingest(&w, &b, &clause(ROOT, NOW - 50, 45), ROOT, 3).await,
        AckResult::Applied
    );
    assert_eq!(held_minutes(&b), Some(45));
    // Rule 5: the view names its root, with no label for a root.
    let view = guardians::build_view(&keys, NOW).unwrap();
    assert_eq!(view.roots, vec![guardians::short_npub(&pk(ROOT))]);
    assert_eq!(view.rules.len(), 1);
    assert_eq!(view.rules[0].label, None);
}

#[tokio::test]
async fn an_approved_working_key_signs_until_it_is_withdrawn() {
    let w = Ward::new("withdraw");
    let keys = w.keys();
    let b = w.broker(&keys);
    // Before its approval lands, W's clause is dropped.
    assert_eq!(
        ingest(&w, &b, &clause(W, NOW - 300, 30), W, 1).await,
        AckResult::Refused
    );
    approve(&w, &b, ROOT, W, 2).await;
    assert_eq!(
        ingest(&w, &b, &clause(W, NOW - 300, 30), W, 3).await,
        AckResult::Applied
    );
    assert_eq!(held_minutes(&b), Some(30));
    // Rule 5: W's label next to its ROOT's short npub.
    let view = guardians::build_view(&keys, NOW).unwrap();
    assert_eq!(
        view.rules[0].set_by(),
        format!("Mum's phone ({})", guardians::short_npub(&pk(ROOT)))
    );
    // The root withdraws W: W's next clause is refused, its rule stays.
    let withdrawal = statement(
        ROOT,
        format!("kintrinsic/revoke/{}", pk(W).to_hex()),
        pk(W),
        NOW - 200,
        json!({}),
    );
    assert_eq!(
        ingest(&w, &b, &withdrawal, ROOT, 4).await,
        AckResult::Applied
    );
    assert_eq!(
        ingest(&w, &b, &clause(W, NOW - 100, 300), W, 5).await,
        AckResult::Refused
    );
    assert_eq!(
        held_minutes(&b),
        Some(30),
        "a departed key's rule stays enforced"
    );
    // The offline unlock no longer answers to W.
    assert_eq!(keys.signer_keys(), vec![pk(ROOT)]);
}

#[tokio::test]
async fn a_two_root_family_is_accepted_and_every_working_key_hears_status() {
    let w = Ward::new("tworoots");
    let keys = w.keys();
    let b = w.broker(&keys);
    add_coguardian(&w, &b, &keys).await;
    assert_eq!(keys.signers().roots().len(), 2);
    // Both parents' installs set rules.
    assert_eq!(
        ingest(&w, &b, &clause(W2, NOW - 20, 25), W2, 10).await,
        AckResult::Applied
    );
    assert_eq!(held_minutes(&b), Some(25));
    assert_eq!(
        ingest(&w, &b, &clause(W, NOW - 10, 35), W, 11).await,
        AckResult::Applied
    );
    assert_eq!(held_minutes(&b), Some(35));
    // STATUS goes to every working key, through the call charterd makes.
    let relay = MockRelayTransport::new();
    let mut t = CharterTransport::new(
        relay.clone(),
        ScriptedEntropy::new(9),
        secret(0x22),
        pk(ROOT),
        vec!["wss://r".into()],
    );
    t.set_signer_set(keys.signers());
    let status = json!({"subject": SUBJECT.to_hex(), "guardians": keys.status_guardians(NOW)});
    t.emit_status_to(
        &status.to_string(),
        NOW,
        &keys.status_recipients(),
        &keys.status_relays(NOW),
    )
    .await;
    for k in [W, W2] {
        let got = relay
            .query(
                &["wss://r".to_string()],
                Filter {
                    kinds: vec![kinds::GIFT_WRAP],
                    p_tags: vec![pk(k)],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(got.len(), 1, "working key {k:#x}");
    }
    let g = keys.status_guardians(NOW).unwrap();
    assert_eq!(g.roots.len(), 2);
    // An offline unlock code for every key in the set.
    assert_eq!(
        guardians::unlock_codes(&secret(0x22), &keys.signer_keys(), "K7QN").len(),
        4
    );

    // RELEASE from one parent is refused while two roots are pinned.
    let out = b
        .ingest_local(&w.sealed(&release(ROOT), ROOT, 20), LOCAL)
        .await;
    assert!(!out.released);
    assert!(b.sys().pairing().load().unwrap().is_some());
    assert!(keys.is_available());
}

#[tokio::test]
async fn a_presence_request_with_nothing_pending_mints_nothing() {
    let w = Ward::new("nopending");
    let keys = w.keys();
    guardians::request_presence(&w.dir, "kintrinsic/guardian/00", NOW).unwrap();
    let served = guardians::serve_presence_request(&w.dir, &keys, NOW, &mut |b| b.fill(1));
    assert!(
        matches!(served, Some(PresenceServed::Refused(_))),
        "{served:?}"
    );
    assert!(!guardians::presence_request_pending(&w.dir));
    assert!(!w.dir.join(guardians::PRESENCE_TOKEN_FILE).exists());
    // A stale request is refused too, and nothing waits forever.
    guardians::request_presence(&w.dir, "x", NOW - 1_000).unwrap();
    assert!(matches!(
        guardians::serve_presence_request(&w.dir, &keys, NOW, &mut |b| b.fill(1)),
        Some(PresenceServed::Refused(_))
    ));
    assert!(guardians::serve_presence_request(&w.dir, &keys, NOW, &mut |b| b.fill(1)).is_none());
}

#[tokio::test]
async fn a_helper_reads_the_state_without_writing_it() {
    let w = Ward::new("peek");
    let keys = w.keys();
    let b = w.broker(&keys);
    approve(&w, &b, ROOT, W, 1).await;
    let state = w.dir.join(charter_sys::guardian_store::GUARDIAN_STATE_FILE);
    let before = std::fs::read(&state).unwrap();
    let peeked = guardians::peek_keys(&w.dir, &pairing(), NOW + 50);
    assert!(peeked.signers().is_signer(&pk(W)));
    assert_eq!(std::fs::read(&state).unwrap(), before);
    // charter-pair re-pairing clears it all.
    guardians::clear_state(&w.dir).unwrap();
    assert!(!state.exists());
}

// ---------------------------------------------------------------------------
// Scan-to-pair (review 2a, L4)
// ---------------------------------------------------------------------------

fn pin_paths(dir: &Path) -> charterd::pair_commit::PinPaths {
    std::fs::create_dir_all(dir.join("limits.d")).unwrap();
    std::fs::write(
        dir.join("limits.d/axel.json"),
        r#"{"limits":{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}"#,
    )
    .unwrap();
    let s = |p: &str| dir.join(p).to_string_lossy().into_owned();
    charterd::pair_commit::PinPaths {
        pairing: s("pairing.json"),
        limits_dir: s("limits.d"),
        token: s("pair-token.json"),
        children_base: dir.to_string_lossy().into_owned(),
        state_dir: dir.to_string_lossy().into_owned(),
    }
}

fn offer(
    sealer: u8,
    token: &str,
    approval: Option<&NostrEvent>,
) -> charter_transport::ReceivedPairOffer {
    let mut content = json!({"token": token, "relays": ["wss://relay.example"], "ts": NOW});
    if let Some(a) = approval {
        content["approval"] = serde_json::to_value(a).unwrap();
    }
    let inner = sign_event(
        &TestGuardian::from_seed(sealer).signer,
        kinds::CHARTER_DEVICE_PAIR_OFFER,
        NOW,
        vec![kinds::marker_tag()],
        content.to_string(),
    );
    charter_transport::ReceivedPairOffer {
        offer: charter_transport::pair_offer::parse_pair_offer(&inner.content).unwrap(),
        seal_author: pk(sealer),
        inner: Some(inner),
    }
}

#[test]
fn an_offer_sealed_by_a_working_key_pins_its_root_and_holds_the_approval() {
    let dir = temp_dir("scan");
    let paths = pin_paths(&dir);
    let token = charterd::pair_token::mint(&paths.token, NOW, [0x5a; 16]).unwrap();
    let ap = approval(ROOT, W, NOW - 10, "Mum's phone");
    let pinned = charterd::pair_listener::try_pair_once(
        &paths,
        machine_pk(),
        &[offer(W, &token, Some(&ap))],
        [0xCC; 32],
        NOW,
    );
    assert_eq!(pinned, Some(pk(ROOT)), "the root, never the working key");
    let record: Pairing =
        serde_json::from_str(&std::fs::read_to_string(&paths.pairing).unwrap()).unwrap();
    assert_eq!(record.guardian_pubkey, pk(ROOT));
    assert_eq!(record.paired_at, NOW, "the decision's paired_at");
    // The next start loads the state written at pairing (no re-migration):
    // W is already a signer bound to R.
    let keys = guardians::load_keys(&dir, &record, NOW + 5);
    let set = keys.signers();
    assert_eq!(set.root_of(&pk(W)), Some(pk(ROOT)));
    assert_eq!(set.roots(), vec![pk(ROOT)]);
    assert_eq!(keys.signer_label(&pk(W)).as_deref(), Some("Mum's phone"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_offer_from_a_working_key_without_a_valid_approval_pins_nobody() {
    let dir = temp_dir("scan-bad");
    let paths = pin_paths(&dir);
    let token = charterd::pair_token::mint(&paths.token, NOW, [0x5a; 16]).unwrap();
    // R's approval names another key: refused, nothing written.
    let other = approval(ROOT, W2, NOW - 10, "x");
    let pinned = charterd::pair_listener::try_pair_once(
        &paths,
        machine_pk(),
        &[offer(W, &token, Some(&other))],
        [0xCC; 32],
        NOW,
    );
    assert_eq!(pinned, None);
    assert!(!Path::new(&paths.pairing).exists());
    assert!(!dir
        .join(charter_sys::guardian_store::GUARDIAN_STATE_FILE)
        .exists());
    // A root's own offer, no approval, pins it as before.
    let pinned = charterd::pair_listener::try_pair_once(
        &paths,
        machine_pk(),
        &[offer(ROOT, &token, None)],
        [0xCC; 32],
        NOW,
    );
    assert_eq!(pinned, Some(pk(ROOT)));
    let _ = std::fs::remove_dir_all(&dir);
}
