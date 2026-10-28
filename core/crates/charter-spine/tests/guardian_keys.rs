//! The guardian-keys seams in the broker: the signer set every author check
//! goes through, the statement port (persist before apply, departures), the
//! signer recorded with each clause and the departure floor (rules 4 and 5),
//! gift-wrapped RELAYS (rule 11), and one wrap per recipient (rule 9). The
//! port here is a scripted stand-in for the guardian-keys state machine.

#![cfg(feature = "mock")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use charter_link::AckResult;
use charter_primitives::{kinds, NostrEvent, PubKey, SignerSet};
use charter_proto::ClauseKind;
use charter_spine::ports::NullEventSink;
use charter_spine::{
    Broker, CharterTransportAdapter, EnactorRegistry, GuardianApplied, GuardianStatements,
    LocalIngestOptions,
};
use charter_sys::persistence::{ChildClauseStore, HeldSigner, MockDisk, PairingStore};
use charter_sys::relay::{Filter, MockRelayTransport, RelayTransport};
use charter_sys::{MockSystem, SysResult, SystemLayer};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::relay_config::{default_relays, PreviousRelays};
use charter_transport::{CharterTransport, GuardianSealed, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, TestGuardian};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);
const ROOT_SEED: u8 = 0x11;
const WORKING_SEED: u8 = 0x12;
const STRANGER_SEED: u8 = 0x33;
const LOCAL: LocalIngestOptions = LocalIngestOptions {
    handle_release: true,
};

type Facade = CharterTransportAdapter<MockRelayTransport, ScriptedEntropy>;
type B = Broker<MockSystem, Facade, ScriptedEntropy>;

fn secret(b: u8) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = b;
    s
}

fn pk(seed: u8) -> PubKey {
    PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret(seed)).unwrap())
}

fn machine_sk() -> [u8; 32] {
    secret(0x22)
}

fn machine_pk() -> PubKey {
    pk(0x22)
}

/// A signer set the test edits: current keys with their roots, and departed
/// keys with the time they left.
#[derive(Default)]
struct Set {
    current: BTreeMap<[u8; 32], PubKey>,
    departed: BTreeMap<[u8; 32], u64>,
    roots: Vec<PubKey>,
}

#[derive(Clone, Default)]
struct LiveSet(Arc<Mutex<Set>>);

impl LiveSet {
    fn with(keys: &[(PubKey, PubKey)]) -> Self {
        let s = LiveSet::default();
        {
            let mut g = s.0.lock().unwrap();
            for (k, root) in keys {
                g.current.insert(*k.as_bytes(), *root);
                if !g.roots.contains(root) {
                    g.roots.push(*root);
                }
            }
        }
        s
    }
    fn depart(&self, key: PubKey, at: u64) {
        let mut g = self.0.lock().unwrap();
        g.current.remove(key.as_bytes());
        g.departed.insert(*key.as_bytes(), at);
    }
}

impl SignerSet for LiveSet {
    fn is_signer(&self, key: &PubKey) -> bool {
        self.0.lock().unwrap().current.contains_key(key.as_bytes())
    }
    fn root_of(&self, key: &PubKey) -> Option<PubKey> {
        self.0.lock().unwrap().current.get(key.as_bytes()).copied()
    }
    fn departed_at(&self, key: &PubKey) -> Option<u64> {
        self.0.lock().unwrap().departed.get(key.as_bytes()).copied()
    }
    fn recipients(&self) -> Vec<PubKey> {
        let g = self.0.lock().unwrap();
        g.current
            .keys()
            .map(|k| PubKey::from_bytes(*k))
            .filter(|k| !g.roots.contains(k))
            .collect()
    }
    fn roots(&self) -> Vec<PubKey> {
        self.0.lock().unwrap().roots.clone()
    }
}

/// The scripted port: records calls in order; on apply, departs the keys in
/// `on_apply_depart`.
#[derive(Default)]
struct Port {
    set: LiveSet,
    calls: Mutex<Vec<String>>,
    fail_persist: Mutex<bool>,
    on_apply_depart: Mutex<Vec<PubKey>>,
    previous: Mutex<Option<PreviousRelays>>,
}

impl GuardianStatements for Port {
    fn signers(&self) -> Arc<dyn SignerSet> {
        Arc::new(self.set.clone())
    }
    fn accepts_statements(&self) -> bool {
        true
    }
    fn persist(&self, inbound: &GuardianSealed, _now: u64) -> SysResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("persist {}", inbound.kind()));
        if *self.fail_persist.lock().unwrap() {
            return Err(charter_sys::SysError::Io("disk full".into()));
        }
        Ok(())
    }
    fn apply(&self, inbound: GuardianSealed, now: u64) -> GuardianApplied {
        self.calls
            .lock()
            .unwrap()
            .push(format!("apply {}", inbound.kind()));
        let departing = std::mem::take(&mut *self.on_apply_depart.lock().unwrap());
        for k in &departing {
            self.set.depart(*k, now);
        }
        GuardianApplied {
            result: AckResult::Applied,
            departures: departing.into_iter().map(|k| (k, now)).collect(),
        }
    }
    fn relays_changed(&self, previous: PreviousRelays, _now: u64) {
        *self.previous.lock().unwrap() = Some(previous);
    }
}

struct Ward {
    disk: MockDisk,
    relay: MockRelayTransport,
}

impl Ward {
    fn new() -> Self {
        Ward {
            disk: MockDisk::new(),
            relay: MockRelayTransport::new(),
        }
    }

    fn broker(&self, port: Option<Arc<Port>>) -> B {
        let t = CharterTransport::new(
            self.relay.clone(),
            ScriptedEntropy::new(7),
            machine_sk(),
            pk(ROOT_SEED),
            vec!["wss://r".into()],
        );
        let b = Broker::new(
            MockSystem::over_disk(NOW, self.disk.clone()),
            CharterTransportAdapter::new(t),
            ScriptedEntropy::new(1),
            EnactorRegistry::new(),
            Box::new(NullEventSink),
            SUBJECT,
        );
        match port {
            Some(p) => b.with_guardians(p),
            None => b,
        }
    }

    /// `ev` sealed by the key with low byte `sealer`, wrapped to the ward.
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

fn clause(seed: u8, issued_at: u64, minutes: u32) -> NostrEvent {
    ClauseBuilder::budget(issued_at)
        .subject(SUBJECT)
        .body(json!({"dailyMinutes": minutes, "tz": "UTC"}))
        .build(&TestGuardian::from_seed(seed))
}

fn statement(seed: u8, d: &str) -> NostrEvent {
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_GUARDIAN_STATEMENT,
        NOW,
        vec![
            kinds::marker_tag(),
            vec!["d".into(), d.into()],
            vec!["p".into(), pk(WORKING_SEED).to_hex()],
        ],
        json!({"v": 1, "issuedAt": NOW}).to_string(),
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

fn held_signer(b: &B) -> HeldSigner {
    b.sys()
        .child_clauses()
        .slot_state(&SUBJECT.to_hex(), ClauseKind::Budget.store_key())
        .unwrap()
        .signer
}

fn two_key_port() -> Arc<Port> {
    Arc::new(Port {
        set: LiveSet::with(&[
            (pk(ROOT_SEED), pk(ROOT_SEED)),
            (pk(WORKING_SEED), pk(ROOT_SEED)),
        ]),
        ..Port::default()
    })
}

#[tokio::test]
async fn single_pinned_records_the_signer_and_refuses_everyone_else() {
    let w = Ward::new();
    let b = w.broker(None);
    let ev = clause(ROOT_SEED, NOW, 60);
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_signer(&b), HeldSigner::Key(pk(ROOT_SEED)));

    let stranger = clause(STRANGER_SEED, NOW + 1, 1);
    let out = b
        .ingest_local(&w.sealed(&stranger, STRANGER_SEED, 2), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(held_minutes(&b), Some(60));
}

#[tokio::test]
async fn a_clause_more_than_300_seconds_ahead_is_refused() {
    let w = Ward::new();
    let b = w.broker(None);
    let ev = clause(ROOT_SEED, NOW + 301, 60);
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Refused);
    let ok = clause(ROOT_SEED, NOW + 300, 60);
    let out = b.ingest_local(&w.sealed(&ok, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
}

#[tokio::test]
async fn without_a_port_a_statement_is_dropped_as_an_unknown_kind() {
    let w = Ward::new();
    let b = w.broker(None);
    // Signed by the root, carried by a working key: only the seal-exempt
    // unwrap opens it, and with no port it is ignored.
    let st = statement(ROOT_SEED, "kintrinsic/guardian/ab");
    let out = b.ingest_local(&w.sealed(&st, WORKING_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Ignored);
    assert_eq!(out.inner_kind, Some(kinds::CHARTER_GUARDIAN_STATEMENT));
}

#[tokio::test]
async fn any_key_in_the_set_signs_and_the_port_persists_before_it_applies() {
    let w = Ward::new();
    let port = two_key_port();
    let b = w.broker(Some(port.clone()));

    let ev = clause(WORKING_SEED, NOW, 45);
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_signer(&b), HeldSigner::Key(pk(WORKING_SEED)));

    // A clause signed by the root but sealed by the working key is not a
    // statement: the strict binding refuses it.
    let resealed = clause(ROOT_SEED, NOW + 5, 10);
    let out = b
        .ingest_local(&w.sealed(&resealed, WORKING_SEED, 2), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);

    let st = statement(ROOT_SEED, "kintrinsic/guardian/ab");
    let out = b.ingest_local(&w.sealed(&st, WORKING_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(
        *port.calls.lock().unwrap(),
        vec!["persist 31120".to_string(), "apply 31120".to_string()]
    );

    *port.fail_persist.lock().unwrap() = true;
    let out = b.ingest_local(&w.sealed(&st, WORKING_SEED, 4), LOCAL).await;
    assert_eq!(out.result, AckResult::Failed);
    assert_eq!(
        port.calls.lock().unwrap().last().map(String::as_str),
        Some("persist 31120"),
        "not applied after a failed persist"
    );
}

#[tokio::test]
async fn a_departed_keys_clause_stays_until_a_current_signer_replaces_it_above_the_floor() {
    let w = Ward::new();
    let port = two_key_port();
    let b = w.broker(Some(port.clone()));

    // The working key sets a rule dated as far ahead as the ward allows.
    let far = clause(WORKING_SEED, NOW + 300, 30);
    let out = b
        .ingest_local(&w.sealed(&far, WORKING_SEED, 1), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);

    // While it is current, an older clause from the root cannot replace it.
    let older = clause(ROOT_SEED, NOW + 10, 90);
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Superseded);

    // The working key departs at NOW (a withdrawal the port applies).
    port.on_apply_depart.lock().unwrap().push(pk(WORKING_SEED));
    let st = statement(ROOT_SEED, "kintrinsic/revoke/ab");
    let out = b.ingest_local(&w.sealed(&st, ROOT_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(!port.set.is_signer(&pk(WORKING_SEED)));

    // Its clause is still enforced...
    assert_eq!(held_minutes(&b), Some(30));
    // ...nothing new from it is accepted...
    let from_departed = clause(WORKING_SEED, NOW + 300, 5);
    let out = b
        .ingest_local(&w.sealed(&from_departed, WORKING_SEED, 4), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    // ...a clause at or below the departure floor (min(NOW + 300, NOW)) is
    // still refused, so a relay cannot restore an older rule...
    let at_floor = clause(ROOT_SEED, NOW, 120);
    let out = b
        .ingest_local(&w.sealed(&at_floor, ROOT_SEED, 5), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Superseded);
    assert_eq!(held_minutes(&b), Some(30));
    // ...and the root's newer clause replaces it although the departed
    // key's issuedAt is higher.
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 6), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_minutes(&b), Some(90));
    assert_eq!(held_signer(&b), HeldSigner::Key(pk(ROOT_SEED)));

    // From here the ordinary replay floor holds again.
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 7), LOCAL).await;
    assert_eq!(out.result, AckResult::Unchanged);
}

#[tokio::test]
async fn a_legacy_record_without_a_signer_is_the_pinned_roots() {
    let w = Ward::new();
    let b = w.broker(Some(two_key_port()));
    b.sys()
        .child_clauses()
        .put_child_clause(
            &SUBJECT.to_hex(),
            ClauseKind::Budget.store_key(),
            NOW,
            r#"{"dailyMinutes":15,"tz":"UTC"}"#,
        )
        .unwrap();
    assert_eq!(held_signer(&b), HeldSigner::Legacy);
    // The pinned root is current, so the record's issuedAt binds.
    let older = clause(WORKING_SEED, NOW - 1, 99);
    let out = b
        .ingest_local(&w.sealed(&older, WORKING_SEED, 1), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Superseded);
}

fn pair(b: &B) {
    b.sys()
        .pairing()
        .save(
            &json!({
                "guardian_pubkey": pk(ROOT_SEED).to_hex(),
                "relays": ["wss://r"],
                "machine": machine_pk().to_hex(),
                "subject_pubkey": SUBJECT.to_hex(),
                "audit_transparency": false,
                "paired_at": NOW - 100,
                "relays_updated_at": 0,
                "a_field_this_build_does_not_know": 7,
            })
            .to_string(),
        )
        .unwrap();
}

fn relays_event(seed: u8, at: u64, body: serde_json::Value) -> NostrEvent {
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_DEVICE_RELAYS,
        at,
        vec![kinds::marker_tag(), vec!["p".into(), machine_pk().to_hex()]],
        body.to_string(),
    )
}

#[tokio::test]
async fn a_gift_wrapped_relays_change_from_a_working_key_applies_with_its_pin() {
    let w = Ward::new();
    let port = two_key_port();
    let b = w.broker(Some(port.clone()));
    pair(&b);
    let ev = relays_event(
        WORKING_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays(),
            "guardianPin": {"v": 1, "salt": "ab".repeat(16), "hash": "cd".repeat(32)}}),
    );
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(out.relays_changed);
    let stored: serde_json::Value =
        serde_json::from_str(&b.sys().pairing().load().unwrap().unwrap()).unwrap();
    assert_eq!(stored["relays"], json!(default_relays()));
    assert_eq!(stored["guardian_pin"]["hash"], "cd".repeat(32));
    assert_eq!(stored["relays_updated_at"], NOW);
    assert_eq!(stored["a_field_this_build_does_not_know"], 7);
    assert_eq!(
        port.previous.lock().unwrap().as_ref().unwrap().relays,
        vec!["wss://r".to_string()]
    );

    // The same change again is unchanged; a stranger's is refused.
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Unchanged);
    let stranger = relays_event(
        STRANGER_SEED,
        NOW + 1,
        json!({"machine": machine_pk(), "relays": default_relays()}),
    );
    let out = b
        .ingest_local(&w.sealed(&stranger, STRANGER_SEED, 3), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
}

#[tokio::test]
async fn a_remote_release_is_refused_while_two_roots_are_pinned() {
    let w = Ward::new();
    let port = Arc::new(Port {
        set: LiveSet::with(&[(pk(ROOT_SEED), pk(ROOT_SEED)), (pk(0x13), pk(0x13))]),
        ..Port::default()
    });
    let b = w.broker(Some(port));
    pair(&b);
    let release = sign_event(
        &TestGuardian::from_seed(ROOT_SEED).signer,
        kinds::CHARTER_DEVICE_RELEASE,
        NOW,
        vec![kinds::marker_tag()],
        json!({"v": 1, "machine": machine_pk().to_hex(), "issuedAt": NOW}).to_string(),
    );
    let out = b
        .ingest_local(&w.sealed(&release, ROOT_SEED, 1), LOCAL)
        .await;
    assert!(!out.released);
    assert!(b.sys().pairing().load().unwrap().is_some());
}

#[tokio::test]
async fn statements_are_polled_from_relays_ahead_of_clauses() {
    let w = Ward::new();
    let port = two_key_port();
    let b = w.broker(Some(port.clone()));
    let st = statement(ROOT_SEED, "kintrinsic/device/ab");
    w.relay
        .publish(&["wss://r".to_string()], w.sealed(&st, WORKING_SEED, 1))
        .await;
    b.poll_once().await;
    assert_eq!(
        *port.calls.lock().unwrap(),
        vec!["persist 31120".to_string(), "apply 31120".to_string()]
    );
}

/// Every wrap the relay holds addressed to `recipient`.
async fn wraps_to(relay: &MockRelayTransport, recipient: PubKey) -> Vec<NostrEvent> {
    relay
        .query(
            &["wss://r".to_string()],
            Filter {
                kinds: vec![kinds::GIFT_WRAP],
                p_tags: vec![recipient],
                ..Default::default()
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn status_goes_to_every_recipient_and_a_single_pinned_set_changes_nothing() {
    let status = r#"{"subject":"bb"}"#;
    // Without a set, and with a set that is just the pinned guardian, the
    // transport builds the same wrap from the same entropy.
    let plain = CharterTransport::new(
        MockRelayTransport::new(),
        ScriptedEntropy::new(9),
        machine_sk(),
        pk(ROOT_SEED),
        vec!["wss://r".into()],
    );
    let mut pinned = CharterTransport::new(
        MockRelayTransport::new(),
        ScriptedEntropy::new(9),
        machine_sk(),
        pk(ROOT_SEED),
        vec!["wss://r".into()],
    );
    pinned.set_signer_set(Arc::new(charter_primitives::SinglePinned(pk(ROOT_SEED))));
    assert_eq!(
        plain.build_status_wrap(status, NOW),
        pinned.build_status_wraps(status, NOW).pop()
    );

    let relay = MockRelayTransport::new();
    let set = LiveSet::with(&[
        (pk(ROOT_SEED), pk(ROOT_SEED)),
        (pk(WORKING_SEED), pk(ROOT_SEED)),
        (pk(0x14), pk(ROOT_SEED)),
    ]);
    let mut t = CharterTransport::new(
        relay.clone(),
        ScriptedEntropy::new(9),
        machine_sk(),
        pk(ROOT_SEED),
        vec!["wss://r".into()],
    );
    t.set_signer_set(Arc::new(set));
    t.emit_status(status, NOW).await;
    assert_eq!(wraps_to(&relay, pk(WORKING_SEED)).await.len(), 1);
    assert_eq!(wraps_to(&relay, pk(0x14)).await.len(), 1);
    assert!(
        wraps_to(&relay, pk(ROOT_SEED)).await.is_empty(),
        "a root with working keys in the set gets nothing directly"
    );
}
