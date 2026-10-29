//! The guardian-keys adapter in the broker: the signer set every author check
//! goes through, statements persisted before they apply, clause acceptance
//! judged by the guardian-keys state (rules 4 and 5, one source of truth),
//! gift-wrapped RELAYS with the guardian PIN kept out of the pairing record
//! (rule 11, review 2a L2), presence confirmation, the RELEASE gate, the
//! guardian PIN, and one wrap per recipient (rule 9). A ward that pins one
//! guardian and has no port keeps today's behaviour.

#![cfg(feature = "mock")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use charter_link::AckResult;
use charter_primitives::{kinds, NostrEvent, PubKey, SignerSet};
use charter_proto::ClauseKind;
use charter_spine::ports::NullEventSink;
use charter_spine::{
    clean_label, Broker, CharterTransportAdapter, EnactorRegistry, GuardianKeys,
    GuardianStatements, LocalIngestOptions, PinCheck, PresenceError, RelaysChange, RelaysRecord,
};
use charter_sys::guardian_store::{GuardianStateStore, MemoryGuardianStateStore};
use charter_sys::persistence::{ChildClauseStore, HeldSigner, MockDisk, PairingStore};
use charter_sys::relay::{Filter, MockRelayTransport, RelayTransport};
use charter_sys::{MockSystem, SystemLayer};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_transport::relay_config::default_relays;
use charter_transport::{CharterTransport, Pairing, ScriptedEntropy};
use charter_verify::test_support::{sign_event, ClauseBuilder, TestGuardian};
use serde_json::json;

const NOW: u64 = 1_700_001_000;
const PAIRED_AT: u64 = NOW - 1_000;
const SUBJECT: PubKey = PubKey::from_bytes([0xBB; 32]);
const ROOT_SEED: u8 = 0x11;
const WORKING_SEED: u8 = 0x12;
const ROOT2_SEED: u8 = 0x13;
const WORKING2_SEED: u8 = 0x14;
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

fn pairing_json() -> serde_json::Value {
    json!({
        "guardian_pubkey": pk(ROOT_SEED).to_hex(),
        "relays": ["wss://r"],
        "machine": machine_pk().to_hex(),
        "subject_pubkey": SUBJECT.to_hex(),
        "audit_transparency": false,
        "paired_at": PAIRED_AT,
        "relays_updated_at": 0,
        "a_field_this_build_does_not_know": 7,
    })
}

fn pairing() -> Pairing {
    serde_json::from_value(pairing_json()).unwrap()
}

struct Ward {
    disk: MockDisk,
    relay: MockRelayTransport,
    store: MemoryGuardianStateStore,
}

impl Ward {
    fn new() -> Self {
        Ward {
            disk: MockDisk::new(),
            relay: MockRelayTransport::new(),
            store: MemoryGuardianStateStore::new(),
        }
    }

    /// The adapter over this ward's store, loaded as a restart would.
    fn keys(&self) -> GuardianKeys {
        GuardianKeys::load(Arc::new(self.store.clone()), &pairing(), NOW)
    }

    fn broker_with(&self, port: Option<Arc<dyn GuardianStatements>>) -> B {
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
        b.sys().pairing().save(&pairing_json().to_string()).unwrap();
        match port {
            Some(p) => b.with_guardians(p),
            None => b,
        }
    }

    /// A broker with the adapter, and the adapter.
    fn broker(&self) -> (B, GuardianKeys) {
        let keys = self.keys();
        (self.broker_with(Some(Arc::new(keys.clone()))), keys)
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

fn statement_by(
    seed: u8,
    d: String,
    p: PubKey,
    issued_at: u64,
    extra: serde_json::Value,
) -> NostrEvent {
    let mut content = json!({"v": 1, "issuedAt": issued_at});
    if let (Some(c), Some(e)) = (content.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            c.insert(k.clone(), v.clone());
        }
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

/// `root` approves working key `w`.
fn approval(root: u8, w: u8, issued_at: u64) -> NostrEvent {
    statement_by(
        root,
        format!("kintrinsic/device/{}", pk(w).to_hex()),
        pk(w),
        issued_at,
        json!({}),
    )
}

/// `root` withdraws working key `w`.
fn withdrawal(root: u8, w: u8, issued_at: u64) -> NostrEvent {
    statement_by(
        root,
        format!("kintrinsic/revoke/{}", pk(w).to_hex()),
        pk(w),
        issued_at,
        json!({}),
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

/// Approve W (sealed by W, proof of possession) on a ward with the adapter.
async fn approve(w: &Ward, b: &B, root: u8, working: u8, salt: u8) {
    let out = b
        .ingest_local(
            &w.sealed(&approval(root, working, NOW - 500), working, salt),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied, "approval of {working:#x}");
}

// ---------------------------------------------------------------------------
// No port: today's behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_pinned_records_the_signer_and_refuses_everyone_else() {
    let w = Ward::new();
    let b = w.broker_with(None);
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
    // The replay floor alone: an older clause is superseded.
    let older = clause(ROOT_SEED, NOW - 1, 5);
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Superseded);
}

#[tokio::test]
async fn a_clause_more_than_300_seconds_ahead_is_refused() {
    let w = Ward::new();
    let b = w.broker_with(None);
    let ev = clause(ROOT_SEED, NOW + 301, 60);
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Refused);
    let ok = clause(ROOT_SEED, NOW + 300, 60);
    let out = b.ingest_local(&w.sealed(&ok, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
}

#[tokio::test]
async fn without_a_port_statements_and_wrapped_relays_are_ignored() {
    let w = Ward::new();
    let b = w.broker_with(None);
    let st = approval(ROOT_SEED, WORKING_SEED, NOW);
    let out = b.ingest_local(&w.sealed(&st, WORKING_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Ignored);
    assert_eq!(out.inner_kind, Some(kinds::CHARTER_GUARDIAN_STATEMENT));
    // Review 2a, Info 1: a gift-wrapped RELAYS is part of the section.
    let ev = relays_event(
        ROOT_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays()}),
    );
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Ignored);
}

// ---------------------------------------------------------------------------
// Migration and persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_already_paired_ward_migrates_in_place_to_its_pinned_root() {
    let w = Ward::new();
    assert!(w.store.raw().is_none());
    let (b, keys) = w.broker();
    // Written at load, before anything is applied.
    assert!(w.store.raw().is_some());
    let set = keys.signers();
    assert!(set.is_signer(&pk(ROOT_SEED)));
    assert!(!set.is_signer(&pk(WORKING_SEED)));
    assert_eq!(set.roots(), vec![pk(ROOT_SEED)]);
    assert_eq!(set.recipients(), vec![pk(ROOT_SEED)]);
    // The root signs as before.
    let out = b
        .ingest_local(&w.sealed(&clause(ROOT_SEED, NOW, 60), ROOT_SEED, 1), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
    // Reloading reads the same state back (no re-migration).
    let saves = w.store.saves();
    let again = w.keys();
    assert!(again.signers().is_signer(&pk(ROOT_SEED)));
    assert_eq!(w.store.saves(), saves);
}

#[tokio::test]
async fn a_working_key_signs_once_approved_and_a_failed_persist_applies_nothing() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    // Before its approval lands, W's clause is dropped, not buffered.
    let ev = clause(WORKING_SEED, NOW, 45);
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Refused);

    approve(&w, &b, ROOT_SEED, WORKING_SEED, 2).await;
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_signer(&b), HeldSigner::Key(pk(WORKING_SEED)));
    // The live handle wired once sees the approval.
    assert_eq!(keys.signers().recipients(), vec![pk(WORKING_SEED)]);

    // The same approval again: unchanged.
    let out = b
        .ingest_local(
            &w.sealed(
                &approval(ROOT_SEED, WORKING_SEED, NOW - 500),
                WORKING_SEED,
                4,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Unchanged);

    // Rule 10: the store refuses the write, so the second key is not applied.
    w.store.break_writes(true);
    let out = b
        .ingest_local(
            &w.sealed(
                &approval(ROOT_SEED, WORKING2_SEED, NOW - 400),
                WORKING2_SEED,
                5,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Failed);
    assert!(!keys.signers().is_signer(&pk(WORKING2_SEED)));
    w.store.break_writes(false);
    // And a restart reads back exactly what was persisted.
    let reloaded = w.keys();
    assert!(reloaded.signers().is_signer(&pk(WORKING_SEED)));
    assert!(!reloaded.signers().is_signer(&pk(WORKING2_SEED)));
}

#[tokio::test]
async fn an_unreadable_state_fails_closed_and_is_left_on_disk() {
    let w = Ward::new();
    w.store.set_raw(Some("{not json".into()));
    let (b, keys) = w.broker();
    assert!(!keys.is_available());
    let set = keys.signers();
    assert!(!set.is_signer(&pk(ROOT_SEED)));
    assert!(set.recipients().is_empty(), "nothing is sent to a guess");
    let out = b
        .ingest_local(&w.sealed(&clause(ROOT_SEED, NOW, 60), ROOT_SEED, 1), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    assert_eq!(held_minutes(&b), None);
    assert_eq!(w.store.raw().as_deref(), Some("{not json"));

    // A transient read error heals at the next advance.
    let w = Ward::new();
    let _ = w.keys(); // migrate, so there is a good document
    w.store.break_reads(true);
    let keys = w.keys();
    assert!(!keys.is_available());
    w.store.break_reads(false);
    keys.advance(NOW);
    assert!(keys.signers().is_signer(&pk(ROOT_SEED)));
}

// ---------------------------------------------------------------------------
// Clauses: one source of truth (review 2a M1, L6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_departed_keys_clause_stays_until_a_current_signer_replaces_it_above_the_floor() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;

    // The working key sets a rule dated as far ahead as the ward allows.
    let far = clause(WORKING_SEED, NOW + 300, 30);
    let out = b
        .ingest_local(&w.sealed(&far, WORKING_SEED, 2), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);

    // While it is current, an older clause from the root cannot replace it.
    let older = clause(ROOT_SEED, NOW + 10, 90);
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Superseded);

    // The root withdraws the working key at NOW.
    let out = b
        .ingest_local(
            &w.sealed(&withdrawal(ROOT_SEED, WORKING_SEED, NOW), ROOT_SEED, 4),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(keys.signers().is_departed(&pk(WORKING_SEED)));

    // Its clause is still enforced...
    assert_eq!(held_minutes(&b), Some(30));
    // ...nothing new from it is accepted...
    let from_departed = clause(WORKING_SEED, NOW + 300, 5);
    let out = b
        .ingest_local(&w.sealed(&from_departed, WORKING_SEED, 5), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    // ...a clause at or below the floor (min(NOW + 300, NOW)) is refused...
    let at_floor = clause(ROOT_SEED, NOW, 120);
    let out = b
        .ingest_local(&w.sealed(&at_floor, ROOT_SEED, 6), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Superseded);
    assert_eq!(held_minutes(&b), Some(30));
    // ...and the root's newer clause replaces it although the departed key's
    // issuedAt is higher: the store's floor comes down with it.
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 7), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_minutes(&b), Some(90));
    assert_eq!(held_signer(&b), HeldSigner::Key(pk(ROOT_SEED)));

    // From here the root's own issuedAt is the bar.
    let out = b.ingest_local(&w.sealed(&older, ROOT_SEED, 8), LOCAL).await;
    assert_eq!(out.result, AckResult::Unchanged);
}

/// Review 2a M1: the departure floor used to live in a separate file that a
/// failed write (or a crash) left stale, so a relay could roll a rule back.
/// It now lives in the guardian-keys state, persisted with the departure.
#[tokio::test]
async fn a_departure_needs_no_clause_store_write_so_no_stale_floor_can_apply() {
    let w = Ward::new();
    let (b, _keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;
    approve(&w, &b, ROOT_SEED, WORKING2_SEED, 2).await;

    // W1 sets 30 min, then departs: its floor is NOW - 300.
    let out = b
        .ingest_local(
            &w.sealed(&clause(WORKING_SEED, NOW - 300, 30), WORKING_SEED, 3),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    let out = b
        .ingest_local(
            &w.sealed(
                &withdrawal(ROOT_SEED, WORKING_SEED, NOW - 290),
                ROOT_SEED,
                4,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    // The root sets 90 min, then W2 tightens to 10.
    let root_clause = clause(ROOT_SEED, NOW - 200, 90);
    let out = b
        .ingest_local(&w.sealed(&root_clause, ROOT_SEED, 5), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
    let out = b
        .ingest_local(
            &w.sealed(&clause(WORKING2_SEED, NOW - 100, 10), WORKING2_SEED, 6),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);

    // W2 departs while the clause store cannot be written.
    w.disk.break_clause_writes();
    let out = b
        .ingest_local(
            &w.sealed(
                &withdrawal(ROOT_SEED, WORKING2_SEED, NOW - 90),
                ROOT_SEED,
                7,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    w.disk.repair_clause_writes();

    // A relay replays the root's older clause: below W2's floor (NOW - 100).
    let out = b
        .ingest_local(&w.sealed(&root_clause, ROOT_SEED, 8), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Superseded);
    assert_eq!(held_minutes(&b), Some(10));
}

#[tokio::test]
async fn a_lost_body_write_is_repaired_by_the_re_delivery() {
    let w = Ward::new();
    let (b, _keys) = w.broker();
    let first = clause(ROOT_SEED, NOW - 10, 60);
    let out = b.ingest_local(&w.sealed(&first, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);

    // The state accepts the next clause; the body write fails after it.
    let next = clause(ROOT_SEED, NOW, 20);
    w.disk.break_clause_writes();
    let out = b.ingest_local(&w.sealed(&next, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Failed);
    w.disk.repair_clause_writes();
    assert_eq!(held_minutes(&b), Some(60));
    // The floor is already raised: the older clause stays out...
    let out = b.ingest_local(&w.sealed(&first, ROOT_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Superseded);
    // ...and the guardian's re-send of the clause itself lands.
    let out = b.ingest_local(&w.sealed(&next, ROOT_SEED, 4), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_minutes(&b), Some(20));
    let out = b.ingest_local(&w.sealed(&next, ROOT_SEED, 5), LOCAL).await;
    assert_eq!(out.result, AckResult::Unchanged);
}

#[tokio::test]
async fn a_slot_from_before_the_upgrade_keeps_its_replay_floor() {
    let w = Ward::new();
    // A record written before the section, with no signer.
    let legacy = MockSystem::over_disk(NOW, w.disk.clone());
    legacy
        .child_clauses()
        .put_child_clause(
            &SUBJECT.to_hex(),
            ClauseKind::Budget.store_key(),
            NOW,
            r#"{"dailyMinutes":15,"tz":"UTC"}"#,
        )
        .unwrap();
    let (b, _keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;
    assert_eq!(held_signer(&b), HeldSigner::Legacy);
    // The pinned root is current, so the record's issuedAt binds.
    let older = clause(WORKING_SEED, NOW - 1, 99);
    let out = b
        .ingest_local(&w.sealed(&older, WORKING_SEED, 2), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Superseded);
    let newer = clause(WORKING_SEED, NOW + 1, 99);
    let out = b
        .ingest_local(&w.sealed(&newer, WORKING_SEED, 3), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
}

/// A slot last written before the section, by a root that has departed
/// before the port first judged the slot, is seeded with the departure floor
/// (rule 4): the departed root's far-future clause can't hold it for ever
/// (review batch 2, core L1).
#[tokio::test]
async fn a_slot_from_before_the_upgrade_takes_the_departure_floor_of_its_signer() {
    let w = Ward::new();
    // Before the upgrade the pinned root R stored a far-future clause (the
    // ward had no 300-second refusal then): 600 minutes a day.
    let legacy = MockSystem::over_disk(NOW, w.disk.clone());
    legacy
        .child_clauses()
        .put_child_clause(
            &SUBJECT.to_hex(),
            ClauseKind::Budget.store_key(),
            NOW + 1_000_000,
            r#"{"dailyMinutes":600,"tz":"UTC"}"#,
        )
        .unwrap();
    let (b, keys) = w.broker();
    // R2 becomes a co-guardian, confirmed in person by W2.
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;
    let cg = statement_by(
        ROOT_SEED,
        format!("kintrinsic/guardian/{}", pk(ROOT2_SEED).to_hex()),
        pk(ROOT2_SEED),
        NOW - 50,
        json!({}),
    );
    let out = b.ingest_local(&w.sealed(&cg, WORKING_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    let d = keys.pending_statements(NOW)[0].d.clone();
    let token = keys
        .mint_presence_token(Some(&d), NOW, &mut fill(0x7e))
        .unwrap();
    let w2_approval = approval(ROOT2_SEED, WORKING2_SEED, NOW - 40);
    let good = confirm(WORKING2_SEED, &token.token, &cg, Some(&w2_approval));
    let out = b
        .ingest_local(&w.sealed(&good, WORKING2_SEED, 4), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
    // R removes itself; R2 remains.
    let rm = statement_by(
        ROOT_SEED,
        format!("kintrinsic/unguardian/{}", pk(ROOT_SEED).to_hex()),
        pk(ROOT_SEED),
        NOW - 10,
        json!({}),
    );
    let out = b.ingest_local(&w.sealed(&rm, ROOT_SEED, 5), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(keys.signers().is_departed(&pk(ROOT_SEED)));
    // A replay from before the departure is still refused...
    let out = b
        .ingest_local(
            &w.sealed(&clause(WORKING2_SEED, NOW - 20, 45), WORKING2_SEED, 6),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Superseded);
    // ...and a clause from a current signer after it replaces R's.
    let out = b
        .ingest_local(
            &w.sealed(&clause(WORKING2_SEED, NOW, 30), WORKING2_SEED, 7),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    assert_eq!(held_minutes(&b), Some(30));
}

// ---------------------------------------------------------------------------
// RELAYS and the guardian PIN
// ---------------------------------------------------------------------------

fn relays_event(seed: u8, at: u64, body: serde_json::Value) -> NostrEvent {
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_DEVICE_RELAYS,
        at,
        vec![kinds::marker_tag(), vec!["p".into(), machine_pk().to_hex()]],
        body.to_string(),
    )
}

const PIN: &str = "4821";

fn pin_record() -> serde_json::Value {
    let salt = [0x5a; 16];
    let hash = charter_guardians::pin_hash(PIN, &salt);
    json!({
        "v": 1,
        "salt": "5a".repeat(16),
        "hash": hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    })
}

#[tokio::test]
async fn a_gift_wrapped_relays_change_keeps_its_pin_out_of_the_pairing_record() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;
    let ev = relays_event(
        WORKING_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays(), "guardianPin": pin_record()}),
    );
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(out.relays_changed);
    let raw = b.sys().pairing().load().unwrap().unwrap();
    let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(stored["relays"], json!(default_relays()));
    assert_eq!(stored["relays_updated_at"], NOW);
    assert_eq!(stored["a_field_this_build_does_not_know"], 7);
    assert!(
        !raw.contains("5a5a5a5a") && stored.get("guardian_pin").is_none(),
        "L2: the PIN record never reaches the pairing record"
    );
    assert!(keys.has_guardian_pin());
    assert!(w.store.raw().unwrap().contains(&"5a".repeat(16)));

    // The same change again is unchanged; a stranger's is refused.
    let out = b.ingest_local(&w.sealed(&ev, WORKING_SEED, 3), LOCAL).await;
    assert_eq!(out.result, AckResult::Unchanged);
    let stranger = relays_event(
        STRANGER_SEED,
        NOW + 1,
        json!({"machine": machine_pk(), "relays": default_relays()}),
    );
    let out = b
        .ingest_local(&w.sealed(&stranger, STRANGER_SEED, 4), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
}

#[tokio::test]
async fn a_relays_change_the_state_cannot_record_leaves_the_pairing_alone() {
    let w = Ward::new();
    let (b, _keys) = w.broker();
    let before = b.sys().pairing().load().unwrap();
    let ev = relays_event(
        ROOT_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays()}),
    );
    w.store.break_writes(true);
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Failed);
    assert_eq!(b.sys().pairing().load().unwrap(), before);
    w.store.break_writes(false);
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
}

#[tokio::test]
async fn the_guardian_pin_backs_off_and_its_counter_survives_a_restart() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    assert_eq!(keys.verify_guardian_pin(PIN, NOW), PinCheck::NoPin);
    let ev = relays_event(
        ROOT_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays(), "guardianPin": pin_record()}),
    );
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);

    for n in 1..=4 {
        assert_eq!(
            keys.verify_guardian_pin("0000", NOW),
            PinCheck::Wrong {
                failures: n,
                next_try_at: NOW as i64
            }
        );
    }
    assert_eq!(
        keys.verify_guardian_pin("0000", NOW),
        PinCheck::Wrong {
            failures: 5,
            next_try_at: NOW as i64 + 60
        }
    );
    // A restart keeps the count: the next try still waits.
    let restarted = w.keys();
    assert_eq!(
        restarted.verify_guardian_pin(PIN, NOW + 59),
        PinCheck::Wait {
            until: NOW as i64 + 60
        }
    );
    // Once the wait is over the right PIN works, and resets the count.
    assert_eq!(
        restarted.verify_guardian_pin(PIN, NOW + 60),
        PinCheck::Correct
    );
    assert_eq!(
        restarted.verify_guardian_pin("0000", NOW + 61),
        PinCheck::Wrong {
            failures: 1,
            next_try_at: NOW as i64 + 61
        }
    );
    // An attempt that cannot be counted is not checked.
    w.store.break_writes(true);
    assert_eq!(
        restarted.verify_guardian_pin(PIN, NOW + 62),
        PinCheck::Unavailable
    );
}

/// PIN entries made at once at the end of a backoff: exactly one is checked
/// and counted, the rest wait (review batch 2, core M1).
#[tokio::test(flavor = "multi_thread")]
async fn pin_entries_made_at_once_pass_the_backoff_one_at_a_time() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    let ev = relays_event(
        ROOT_SEED,
        NOW,
        json!({"machine": machine_pk(), "relays": default_relays(), "guardianPin": pin_record()}),
    );
    let out = b.ingest_local(&w.sealed(&ev, ROOT_SEED, 1), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    for _ in 0..5 {
        keys.verify_guardian_pin("0000", NOW);
    }
    // At NOW+60 the wait is over: one try is allowed, then 120 s more.
    const N: usize = 8;
    let barrier = Arc::new(std::sync::Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let k = keys.clone();
            let bar = barrier.clone();
            std::thread::spawn(move || {
                bar.wait();
                k.verify_guardian_pin(&format!("{:04}", 1000 + i), NOW + 60)
            })
        })
        .collect();
    let results: Vec<PinCheck> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let checked = results
        .iter()
        .filter(|r| matches!(r, PinCheck::Wrong { .. }))
        .count();
    assert_eq!(checked, 1, "{results:?}");
    assert_eq!(
        results
            .iter()
            .filter(|r| **r
                == PinCheck::Wait {
                    until: NOW as i64 + 60 + 120
                })
            .count(),
        N - 1,
        "{results:?}"
    );
    let raw: serde_json::Value = serde_json::from_str(&w.store.raw().unwrap()).unwrap();
    assert_eq!(raw["pinAttempts"]["failures"], 6);
}

// ---------------------------------------------------------------------------
// Presence confirmation, co-guardians and RELEASE
// ---------------------------------------------------------------------------

fn fill(b: u8) -> impl FnMut(&mut [u8]) {
    move |buf: &mut [u8]| buf.fill(b)
}

fn confirm(
    w: u8,
    token: &str,
    statement: &NostrEvent,
    approval: Option<&NostrEvent>,
) -> NostrEvent {
    let mut content = json!({"v": 1, "token": token, "statement": statement.id.to_hex()});
    if let Some(a) = approval {
        content["approval"] = serde_json::to_value(a).unwrap();
    }
    sign_event(
        &TestGuardian::from_seed(w).signer,
        kinds::CHARTER_PRESENCE_CONFIRM,
        NOW,
        vec![kinds::marker_tag()],
        content.to_string(),
    )
}

/// R approves W; R names R2 a co-guardian (sealed by W, exception 2); the
/// ward mints a token for it; W2, approved by R2, confirms in person.
async fn add_coguardian(w: &Ward, b: &B, keys: &GuardianKeys) {
    approve(w, b, ROOT_SEED, WORKING_SEED, 1).await;
    let cg = statement_by(
        ROOT_SEED,
        format!("kintrinsic/guardian/{}", pk(ROOT2_SEED).to_hex()),
        pk(ROOT2_SEED),
        NOW - 50,
        json!({}),
    );
    let out = b.ingest_local(&w.sealed(&cg, WORKING_SEED, 2), LOCAL).await;
    assert_eq!(out.result, AckResult::Applied);
    let pending = keys.pending_statements(NOW);
    assert_eq!(pending.len(), 1);
    let d = pending[0].d.clone();

    // A token for something not pending is refused.
    assert_eq!(
        keys.mint_presence_token(Some("kintrinsic/guardian/00"), NOW, &mut fill(9)),
        Err(PresenceError::NotPending)
    );
    let token = keys
        .mint_presence_token(Some(&d), NOW, &mut fill(0x7e))
        .unwrap();
    assert_eq!(token.token, "7e".repeat(16));
    assert_eq!(token.expires_at, NOW + 600);

    // A wrong token is refused and spends nothing.
    let w2_approval = approval(ROOT2_SEED, WORKING2_SEED, NOW - 40);
    let bad = confirm(WORKING2_SEED, &"00".repeat(16), &cg, Some(&w2_approval));
    let out = b
        .ingest_local(&w.sealed(&bad, WORKING2_SEED, 3), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Refused);
    let good = confirm(WORKING2_SEED, &token.token, &cg, Some(&w2_approval));
    let out = b
        .ingest_local(&w.sealed(&good, WORKING2_SEED, 4), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
    // Single use.
    let out = b
        .ingest_local(&w.sealed(&good, WORKING2_SEED, 5), LOCAL)
        .await;
    assert_ne!(out.result, AckResult::Applied);
}

#[tokio::test]
async fn a_co_guardian_confirmed_in_person_has_equal_authority() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    add_coguardian(&w, &b, &keys).await;
    let set = keys.signers();
    assert_eq!(set.roots().len(), 2);
    assert_eq!(set.root_of(&pk(WORKING2_SEED)), Some(pk(ROOT2_SEED)));
    let mut r = set.recipients();
    r.sort();
    let mut want = vec![pk(WORKING_SEED), pk(WORKING2_SEED)];
    want.sort();
    assert_eq!(r, want);
    // W2 sets a rule.
    let out = b
        .ingest_local(
            &w.sealed(&clause(WORKING2_SEED, NOW, 25), WORKING2_SEED, 9),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    let g = keys.status_guardians(NOW).unwrap();
    assert_eq!(g.roots.len(), 2);
    assert!(g.pending.is_empty());
}

fn release(seed: u8, issued_at: u64) -> NostrEvent {
    sign_event(
        &TestGuardian::from_seed(seed).signer,
        kinds::CHARTER_DEVICE_RELEASE,
        issued_at,
        vec![kinds::marker_tag()],
        json!({"v": 1, "machine": machine_pk().to_hex(), "issuedAt": issued_at}).to_string(),
    )
}

#[tokio::test]
async fn a_remote_release_is_refused_while_two_roots_are_pinned() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    add_coguardian(&w, &b, &keys).await;
    let out = b
        .ingest_local(&w.sealed(&release(ROOT_SEED, NOW), ROOT_SEED, 20), LOCAL)
        .await;
    assert!(!out.released);
    assert!(b.sys().pairing().load().unwrap().is_some());
    assert!(keys.is_available());
}

#[tokio::test]
async fn a_release_from_a_working_key_unpairs_and_clears_the_state() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 1).await;
    // Before the pairing: refused.
    let out = b
        .ingest_local(
            &w.sealed(&release(WORKING_SEED, PAIRED_AT - 1), WORKING_SEED, 2),
            LOCAL,
        )
        .await;
    assert!(!out.released);
    let out = b
        .ingest_local(
            &w.sealed(&release(WORKING_SEED, NOW), WORKING_SEED, 3),
            LOCAL,
        )
        .await;
    assert!(out.released);
    assert!(b.sys().pairing().load().unwrap().is_none());
    assert!(w.store.raw().is_none());
    assert!(!keys.signers().is_signer(&pk(ROOT_SEED)));
}

#[tokio::test]
async fn statements_are_polled_from_relays_ahead_of_clauses() {
    let w = Ward::new();
    let (b, _keys) = w.broker();
    // W's first clause and its approval arrive in the same round.
    w.relay
        .publish(
            &["wss://r".to_string()],
            w.sealed(&clause(WORKING_SEED, NOW, 40), WORKING_SEED, 1),
        )
        .await;
    w.relay
        .publish(
            &["wss://r".to_string()],
            w.sealed(
                &approval(ROOT_SEED, WORKING_SEED, NOW - 500),
                WORKING_SEED,
                2,
            ),
        )
        .await;
    b.poll_once().await;
    assert_eq!(held_minutes(&b), Some(40));
}

// ---------------------------------------------------------------------------
// Recipients (rule 9)
// ---------------------------------------------------------------------------

/// A signer set the test edits.
#[derive(Default)]
struct Set {
    current: BTreeMap<[u8; 32], PubKey>,
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
}

impl SignerSet for LiveSet {
    fn is_signer(&self, key: &PubKey) -> bool {
        self.0.lock().unwrap().current.contains_key(key.as_bytes())
    }
    fn root_of(&self, key: &PubKey) -> Option<PubKey> {
        self.0.lock().unwrap().current.get(key.as_bytes()).copied()
    }
    fn is_departed(&self, _key: &PubKey) -> bool {
        false
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

#[tokio::test]
async fn a_set_with_no_recipients_sends_nothing_rather_than_fall_back() {
    let w = Ward::new();
    w.store.set_raw(Some("junk".into()));
    let keys = w.keys();
    let mut t = CharterTransport::new(
        w.relay.clone(),
        ScriptedEntropy::new(9),
        machine_sk(),
        pk(ROOT_SEED),
        vec!["wss://r".into()],
    );
    t.set_signer_set(keys.signers());
    assert!(t.recipients().is_empty());
    assert!(t.build_status_wraps("{}", NOW).is_empty());
    assert!(w.store.load().unwrap().is_some());
}

/// A departed working key is owed one last STATUS that shows its departure:
/// [`GuardianKeys::farewell`] names it (the signer set's recipients never
/// do), its body shows who the ward trusts and nothing else (no usage, no
/// relay list), and once marked it is not owed again. A key that departs
/// after the farewell was read stays owed (review batch 2, core L3).
#[tokio::test]
async fn a_departed_key_gets_one_minimal_last_status_then_nothing() {
    let w = Ward::new();
    let (b, keys) = w.broker();
    approve(&w, &b, ROOT_SEED, WORKING_SEED, 0x01).await;
    approve(&w, &b, ROOT_SEED, WORKING2_SEED, 0x03).await;
    assert_eq!(keys.farewell(), None, "nobody has departed");
    let out = b
        .ingest_local(
            &w.sealed(
                &withdrawal(ROOT_SEED, WORKING_SEED, NOW - 400),
                ROOT_SEED,
                0x02,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    assert!(!keys.signers().is_signer(&pk(WORKING_SEED)));
    assert!(!keys.signers().recipients().contains(&pk(WORKING_SEED)));
    let farewell = keys.farewell().expect("owed its last STATUS");
    assert_eq!(farewell.recipients, vec![pk(WORKING_SEED)]);

    let body: serde_json::Value =
        serde_json::from_str(&farewell.to_json(&machine_pk(), NOW, Some(7))).unwrap();
    let fields: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(fields, ["guardians", "machine", "seq", "ts", "v"]);
    let shown: Vec<&str> = body["guardians"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(shown, ["pending", "roots", "tombstones"], "no relay list");
    assert_eq!(body["machine"], machine_pk().to_hex());
    assert_eq!(body["seq"], 7);
    assert_eq!(body["guardians"]["tombstones"], 1);
    assert_eq!(
        body["guardians"]["roots"],
        json!([{"root": pk(ROOT_SEED).to_hex(), "keys": [pk(WORKING2_SEED).to_hex()]}]),
        "the departure shows as the key's absence"
    );

    let relay = MockRelayTransport::new();
    let mut t = CharterTransport::new(
        relay.clone(),
        ScriptedEntropy::new(9),
        machine_sk(),
        pk(ROOT_SEED),
        vec!["wss://r".into()],
    );
    t.set_signer_set(keys.signers());
    t.emit_status_to("{}", NOW, &farewell.recipients, &[]).await;
    assert_eq!(wraps_to(&relay, pk(WORKING_SEED)).await.len(), 1);

    // W2 departs while the farewell to W1 is out: marking W1 leaves W2 owed.
    let out = b
        .ingest_local(
            &w.sealed(
                &withdrawal(ROOT_SEED, WORKING2_SEED, NOW - 300),
                ROOT_SEED,
                0x04,
            ),
            LOCAL,
        )
        .await;
    assert_eq!(out.result, AckResult::Applied);
    keys.mark_status_sent(&farewell.recipients, NOW).unwrap();
    let next = keys.farewell().expect("W2 is still owed its own");
    assert_eq!(next.recipients, vec![pk(WORKING2_SEED)]);
    keys.mark_status_sent(&next.recipients, NOW).unwrap();
    assert_eq!(keys.farewell(), None);
    // Nobody to send to: nothing is built, nothing is published.
    let none = t.emit_status_to("{}", NOW, &[], &[]).await;
    assert!(matches!(
        none.as_slice(),
        [(_, charter_sys::relay::PublishOutcome::Failed(_))]
    ));
}

/// A pasted relay code changes the pairing record while the ward runs: the
/// LIVE adapter follows it (no second instance), persists it, and opens the
/// previous-relays window; a record from another pairing is not followed.
#[tokio::test]
async fn the_live_adapter_follows_a_changed_pairing_record() {
    let w = Ward::new();
    let keys = w.keys();
    let mut changed = pairing();
    changed.relays = vec!["wss://new.example".into()];
    changed.relays_updated_at = Some(NOW - 10);
    let saves = w.store.saves();
    assert_eq!(keys.follow_pairing(&changed, NOW), Ok(true));
    assert_eq!(w.store.saves(), saves + 1, "persisted");
    let g = keys.status_guardians(NOW).unwrap();
    assert_eq!(g.relays, vec!["wss://new.example".to_string()]);
    // Following the same record again changes nothing.
    assert_eq!(keys.follow_pairing(&changed, NOW), Ok(false));
    // A restart reads what was followed, with no relay change to record.
    let restarted = GuardianKeys::load(Arc::new(w.store.clone()), &changed, NOW);
    let p = restarted.project(NOW).unwrap();
    assert_eq!(p.relays, vec!["wss://new.example".to_string()]);
    // Another pairing's record is a re-pair, not a relay change.
    let mut other = changed.clone();
    other.paired_at += 1;
    other.relays = vec!["wss://other.example".into()];
    assert_eq!(keys.follow_pairing(&other, NOW), Ok(false));
    // A write failure changes nothing.
    w.store.break_writes(true);
    let mut third = changed.clone();
    third.relays = vec!["wss://third.example".into()];
    assert!(keys.follow_pairing(&third, NOW).is_err());
    assert_eq!(
        keys.status_guardians(NOW).unwrap().relays,
        vec!["wss://new.example".to_string()]
    );
}

/// A document is matched to its pairing by `pairedAt` and the pinned
/// guardian. An earlier pairing's is replaced by a migration; one with the
/// same `pairedAt` and another guardian, or a newer one, fails closed and is
/// left on disk (review batch 2, core L4).
#[test]
fn a_document_from_another_pairing_fails_closed_unless_it_is_older() {
    let w = Ward::new();
    let keys = w.keys();
    assert!(keys.is_available());
    let doc = w.store.raw().unwrap();

    let mut other_guardian = pairing();
    other_guardian.guardian_pubkey = pk(STRANGER_SEED);
    let k = GuardianKeys::load(Arc::new(w.store.clone()), &other_guardian, NOW);
    assert!(!k.is_available(), "same pairedAt, another guardian");
    assert_eq!(w.store.raw().unwrap(), doc, "left on disk");

    let mut older_record = pairing();
    older_record.paired_at -= 1;
    let k = GuardianKeys::load(Arc::new(w.store.clone()), &older_record, NOW);
    assert!(!k.is_available(), "a document newer than the record");
    assert_eq!(w.store.raw().unwrap(), doc, "left on disk");

    let mut newer_record = other_guardian.clone();
    newer_record.paired_at += 1;
    let k = GuardianKeys::load(Arc::new(w.store.clone()), &newer_record, NOW);
    assert!(k.is_available(), "a re-pair replaced the earlier pairing");
    assert_eq!(k.signers().roots(), vec![pk(STRANGER_SEED)]);

    // A document written before the guardian was recorded matches on
    // `pairedAt` alone.
    let mut legacy: serde_json::Value = serde_json::from_str(&doc).unwrap();
    legacy.as_object_mut().unwrap().remove("pairedGuardian");
    w.store.set_raw(Some(legacy.to_string()));
    assert!(w.keys().is_available());
}

/// A relay change bound for a pairing record the adapter does not serve (a
/// re-pair replaced it while the warden runs) is refused (review batch 2,
/// platforms L5).
#[test]
fn a_relays_change_for_another_pairing_record_is_refused() {
    let w = Ward::new();
    let keys = w.keys();
    let relays = vec!["wss://new.example".to_string()];
    let change = |paired_at| RelaysChange {
        author: pk(ROOT_SEED),
        seal_author: pk(ROOT_SEED),
        ts: NOW,
        relays: &relays,
        pin: None,
        paired_at,
    };
    assert_eq!(
        keys.record_relays(&change(PAIRED_AT + 1), NOW),
        RelaysRecord::Refused
    );
    assert_eq!(
        keys.record_relays(&change(PAIRED_AT), NOW),
        RelaysRecord::Recorded
    );
}

/// A label is shown beside its root's npub: it can't carry a line of its
/// own, a bidi override or an invisible mark (review batch 2, platforms L7).
#[test]
fn a_label_is_cleaned_before_it_is_shown() {
    assert_eq!(clean_label("  Mum  "), "Mum");
    assert_eq!(
        clean_label("Mum (npub1abc)\n1 Kintrinsic app linked\u{2028}x"),
        "Mum (npub1abc)1 Kintrinsic app linkedx"
    );
    assert_eq!(clean_label("\u{202E}muM\u{2066}\u{200B}\u{FEFF}"), "muM");
    let long = "\u{e9}".repeat(40);
    let cut = clean_label(&long);
    assert_eq!(cut.len(), 64);
    assert!(cut.chars().all(|c| c == '\u{e9}'));
    assert_eq!(clean_label("\u{202E}\n"), "");
}

/// `peek` reads the state another process writes and never writes it.
#[tokio::test]
async fn a_peek_reads_the_signer_set_and_labels_and_never_writes() {
    let w = Ward::new();
    let (b, _keys) = w.broker();
    let labelled = statement_by(
        ROOT_SEED,
        format!("kintrinsic/device/{}", pk(WORKING_SEED).to_hex()),
        pk(WORKING_SEED),
        NOW - 500,
        json!({"label": "Mum's phone"}),
    );
    let out = b
        .ingest_local(&w.sealed(&labelled, WORKING_SEED, 0x03), LOCAL)
        .await;
    assert_eq!(out.result, AckResult::Applied);
    let saves = w.store.saves();
    let peeked = GuardianKeys::peek(Arc::new(w.store.clone()), &pairing(), NOW);
    let mut want = vec![pk(ROOT_SEED), pk(WORKING_SEED)];
    want.sort_by_key(|k| k.to_hex());
    let mut got = peeked.signer_keys();
    got.sort_by_key(|k| k.to_hex());
    assert_eq!(got, want);
    assert_eq!(
        peeked.signer_label(&pk(WORKING_SEED)).as_deref(),
        Some("Mum's phone")
    );
    assert_eq!(peeked.signer_label(&pk(ROOT_SEED)), None);
    assert_eq!(
        peeked.mint_presence_token(None, NOW, &mut fill(1)),
        Err(PresenceError::Failed),
        "read-only: nothing can be recorded"
    );
    assert_eq!(w.store.saves(), saves, "a peek writes nothing");

    // A ward with no state yet: the peek migrates in memory only.
    let fresh = Ward::new();
    let peeked = GuardianKeys::peek(Arc::new(fresh.store.clone()), &pairing(), NOW);
    assert_eq!(peeked.signer_keys(), vec![pk(ROOT_SEED)]);
    assert!(fresh.store.raw().is_none());
}

// ---------------------------------------------------------------------------
// Scan-to-pair (review 2a, L4)
// ---------------------------------------------------------------------------

const PAIR_TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn offer(
    sealer: u8,
    approval: Option<NostrEvent>,
    signed: bool,
) -> charter_transport::ReceivedPairOffer {
    let mut content = json!({"token": PAIR_TOKEN, "relays": ["wss://r"], "ts": NOW});
    if let Some(a) = &approval {
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
        inner: signed.then_some(inner),
    }
}

fn token(minted_at: u64) -> charter_guardians::PairToken<'static> {
    charter_guardians::PairToken {
        token: PAIR_TOKEN,
        minted_at: minted_at as i64,
    }
}

#[test]
fn an_offer_sealed_by_a_working_key_pins_its_root_not_the_working_key() {
    let ap = approval(ROOT_SEED, WORKING_SEED, NOW - 10);
    let got = charter_spine::scan_to_pair(&offer(WORKING_SEED, Some(ap), true), &token(NOW), NOW)
        .unwrap();
    assert_eq!(got.root, pk(ROOT_SEED));
    assert_eq!(got.working_key, Some(pk(WORKING_SEED)));
    assert_eq!(got.paired_at(), NOW);
    let store = MemoryGuardianStateStore::new();
    let keys = GuardianKeys::from_paired(Arc::new(store.clone()), got).unwrap();
    assert!(store.raw().is_some());
    let set = keys.signers();
    assert_eq!(set.roots(), vec![pk(ROOT_SEED)]);
    assert_eq!(set.root_of(&pk(WORKING_SEED)), Some(pk(ROOT_SEED)));

    // An approval for another key, or one without a signed offer, is refused.
    let other = approval(ROOT_SEED, WORKING2_SEED, NOW - 10);
    assert!(
        charter_spine::scan_to_pair(&offer(WORKING_SEED, Some(other), true), &token(NOW), NOW)
            .is_err()
    );
    let ap = approval(ROOT_SEED, WORKING_SEED, NOW - 10);
    assert_eq!(
        charter_spine::scan_to_pair(&offer(WORKING_SEED, Some(ap), false), &token(NOW), NOW),
        Err(charter_guardians::Reason::BadOffer)
    );
}

#[test]
fn an_offer_with_no_approval_pins_its_sealer_signed_or_not() {
    for signed in [true, false] {
        let got =
            charter_spine::scan_to_pair(&offer(ROOT_SEED, None, signed), &token(NOW), NOW + 5)
                .unwrap();
        assert_eq!(got.root, pk(ROOT_SEED));
        assert_eq!(got.working_key, None);
        // The token rules hold on both paths.
        let bad = charter_guardians::PairToken {
            token: "ffffffffffffffffffffffffffffffff",
            minted_at: NOW as i64,
        };
        assert_eq!(
            charter_spine::scan_to_pair(&offer(ROOT_SEED, None, signed), &bad, NOW),
            Err(charter_guardians::Reason::TokenInvalid)
        );
        assert_eq!(
            charter_spine::scan_to_pair(&offer(ROOT_SEED, None, signed), &token(NOW - 600), NOW),
            Err(charter_guardians::Reason::TokenInvalid)
        );
    }
}

#[test]
fn the_release_gate_checks_the_seal_and_the_root_count() {
    let machine = machine_pk();
    let ev = release(ROOT_SEED, NOW);
    let one = charter_primitives::SinglePinned(pk(ROOT_SEED));
    assert!(
        charter_spine::release_gate(&ev, &pk(ROOT_SEED), &one, &machine, PAIRED_AT, NOW).is_ok()
    );
    assert_eq!(
        charter_spine::release_gate(&ev, &pk(WORKING_SEED), &one, &machine, PAIRED_AT, NOW),
        Err(charter_spine::ReleaseRefused::SealMismatch)
    );
    let two = LiveSet::with(&[
        (pk(ROOT_SEED), pk(ROOT_SEED)),
        (pk(ROOT2_SEED), pk(ROOT2_SEED)),
    ]);
    assert_eq!(
        charter_spine::release_gate(&ev, &pk(ROOT_SEED), &two, &machine, PAIRED_AT, NOW),
        Err(charter_spine::ReleaseRefused::MoreThanOneRoot)
    );
    assert_eq!(
        charter_spine::release_gate(&ev, &pk(ROOT_SEED), &one, &machine, NOW + 1, NOW),
        Err(charter_spine::ReleaseRefused::BeforePairing)
    );
}
