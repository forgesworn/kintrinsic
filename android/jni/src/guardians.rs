//! The Android ward's side of *Guardian roots and working keys (v1)*
//! (`spec/contract.md`): the guardian-keys port's lifecycle, the clause gate
//! the direct ingest shares with the broker, pasted relay codes, the guardian
//! PIN, presence tokens and the guardian list the ward's screens show.
//!
//! A child module of `warden`, so it reaches the warden's private state.
//!
//! The PIN check runs scrypt (N = 2^15): [`pin_check_json`] and
//! [`mint_presence_json`] take a [`GuardianKeys`] handle, so the JNI layer
//! clones it out of the warden and runs them with the global warden lock
//! RELEASED (an enforcement tick never waits on a PIN entry).

use std::sync::Arc;

use charter_primitives::{NostrEvent, PubKey, SignerSet, SinglePinned};
use charter_proto::ClauseKind;
use charter_spine::ports::{ClauseCandidate, ClauseGate, SlotSeed};
use charter_spine::{GuardianKeys, GuardianStatements, PinCheck, PresenceError};
use charter_sys::guardian_store::{FileGuardianStateStore, GuardianStateStore};
use charter_sys::persistence::{ChildClauseStore, PairingStore, ReplayFloor};
use charter_sys::signer::MachineSigner;
use charter_transport::pairing::Pairing;
use serde_json::{json, Value};

use super::Warden;
use crate::dto;

/// The ward's wall clock, for the paths that are not handed one.
pub(super) fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The signer set of a warden with no pairing: nobody.
struct NoSigners;

impl SignerSet for NoSigners {
    fn is_signer(&self, _key: &PubKey) -> bool {
        false
    }
    fn root_of(&self, _key: &PubKey) -> Option<PubKey> {
        None
    }
    fn is_departed(&self, _key: &PubKey) -> bool {
        false
    }
    fn recipients(&self) -> Vec<PubKey> {
        Vec::new()
    }
    fn roots(&self) -> Vec<PubKey> {
        Vec::new()
    }
}

impl Warden {
    /// (Re)load the guardian-keys port for the current pairing from the
    /// Device Owner's private base dir. A ward paired before the section (or
    /// by a `bunker://` link, which writes no state) is migrated in place:
    /// its pinned guardian becomes its one root. A state file that will not
    /// read fails closed (an empty signer set, no recipients); see
    /// `charter_spine::guardian_keys`.
    pub(super) fn load_guardian_keys(&mut self, now: u64) {
        self.guardian_keys = None;
        let Some(guardian) = self.guardian else {
            return;
        };
        let machine = self.signer.pubkey();
        let pairing = match self.pairing.load() {
            Ok(Some(json)) => serde_json::from_str::<Pairing>(&json).ok(),
            _ => None,
        }
        // The legacy `{guardian, subject}` record: no relays, no pairing time.
        .unwrap_or_else(|| Pairing {
            guardian_pubkey: guardian,
            relays: self.relays.clone(),
            machine,
            subject_pubkey: self.subject.unwrap_or(machine),
            audit_transparency: false,
            paired_at: self.paired_at,
            relays_updated_at: None,
        });
        let store = Arc::new(FileGuardianStateStore::in_dir(&self.base));
        self.guardian_keys = Some(GuardianKeys::load(store, &pairing, now));
    }

    /// The signer set every author check on this ward uses: the port's live
    /// set once a pairing is known.
    pub(super) fn signer_set(&self) -> Arc<dyn SignerSet> {
        match (&self.guardian_keys, self.guardian) {
            (Some(keys), _) => keys.signers(),
            (None, Some(g)) => Arc::new(SinglePinned(g)),
            (None, None) => Arc::new(NoSigners),
        }
    }

    /// A gift-wrapped RELAYS change rewrote the pairing record: take up its
    /// relay list and rebuild the transports on it.
    pub(super) fn reload_relays(&mut self) {
        self.relays_reload_due = false;
        match super::load_pairing(&self.pairing) {
            Ok((guardian, _, relays, _)) if guardian.is_some() && guardian == self.guardian => {
                if relays != self.relays {
                    self.relays = relays;
                    self.last_status = None;
                    self.rebuild_relay();
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("charter: relay change not taken up ({e}); retrying"),
        }
    }

    /// A pasted relay code (About → Change relays). Any key in the signer
    /// set may sign one (contract, *Other places that name the guardian*);
    /// a code that carries a `guardianPin` is refused whole, since a pasted
    /// code passes through whoever pastes it (open question "How a guardian
    /// PIN reaches an Android ward").
    pub(super) fn apply_pasted_relays(
        &self,
        pairing: &mut Pairing,
        code: &str,
        now: u64,
    ) -> Result<(), String> {
        match &self.guardian_keys {
            // Review 2a, Info 6: checked against the live signer set, so a
            // working key may sign one and a moved-away root may not.
            Some(keys) => charter_transport::relay_config::apply_signed_config_from(
                pairing,
                code,
                &*keys.signers(),
                now,
            ),
            None => charter_transport::relay_config::apply_signed_config(pairing, code, now),
        }
    }

    /// The broker's clause gate for a per-child clause (rules 4 and 5),
    /// for the direct ingest: authenticated with no floor against the live
    /// signer set, judged by the guardian-keys state against the clause it
    /// holds for `(subject, kind)` (seeded from the store for a slot it has
    /// never seen), and only then written with its signer. Mirrors
    /// `Broker::judged_clause_disposition`; it runs only where no broker (and
    /// so no local link) is ingesting beside it.
    pub(super) fn judged_clause(
        &self,
        keys: &GuardianKeys,
        event: &NostrEvent,
        seal_author: PubKey,
        kind: ClauseKind,
        subject_hex: &str,
        now: u64,
    ) -> dto::ClauseResult {
        let reject = |reason: String| dto::ClauseResult {
            accepted: false,
            kind: None,
            subject: None,
            reason,
        };
        // Timers first: a removal whose notice has ended is in force before
        // this clause is judged.
        keys.advance(now);
        let signers = keys.signers();
        let vc = match charter_verify::verify_clause(event, &*signers, kind, None, now) {
            Ok(vc) => vc,
            Err(e) => return reject(format!("verify failed: {e:?}")),
        };
        let store_key = kind.store_key();
        // A record with no signer recorded is the pinned root's (rule 5).
        let legacy = self.guardian.unwrap_or(vc.signer());
        let seed = match self.child_clauses.slot_state(subject_hex, store_key) {
            Ok(st) => match (
                st.floor.and_then(ReplayFloor::verify_bound),
                st.signer.resolve(&legacy),
            ) {
                (Some(bound), Some(signer)) => SlotSeed::Held { signer, bound },
                (Some(bound), None) => SlotSeed::Held {
                    signer: legacy,
                    bound,
                },
                (None, _) => SlotSeed::Empty,
            },
            Err(_) => SlotSeed::Unreadable,
        };
        let kind_name = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let candidate = ClauseCandidate {
            author: vc.signer(),
            seal_author,
            subject_hex,
            kind: &kind_name,
            issued_at: vc.issued_at(),
        };
        let body = serde_json::to_string(vc.body()).unwrap_or_default();
        let stored = || dto::ClauseResult {
            accepted: true,
            kind: Some(format!("{kind:?}")),
            subject: Some(subject_hex.to_owned()),
            reason: "stored".into(),
        };
        let write = || match self.child_clauses.replace_child_clause(
            subject_hex,
            store_key,
            vc.issued_at(),
            &vc.signer(),
            &body,
        ) {
            Ok(()) => stored(),
            Err(e) => reject(format!("store error: {e:?}")),
        };
        match keys.judge_clause(&candidate, seed, now) {
            Some(ClauseGate::Accepted) => write(),
            Some(ClauseGate::NotNewer { held_is_this: true }) => {
                // The state holds exactly this clause; the store may have
                // lost its write after the state's.
                let stored_here = self
                    .child_clauses
                    .slot_state(subject_hex, store_key)
                    .ok()
                    .is_some_and(|st| {
                        st.floor == Some(ReplayFloor::Record(vc.issued_at()))
                            && st.signer.resolve(&legacy) == Some(vc.signer())
                    });
                if stored_here {
                    reject("unchanged (already held)".into())
                } else {
                    write()
                }
            }
            Some(ClauseGate::NotNewer {
                held_is_this: false,
            }) => reject("rollback (stale issuedAt)".into()),
            Some(ClauseGate::Refused) => reject("refused by the guardian-keys rules".into()),
            Some(ClauseGate::Unknown) => reject("replay floor unreadable".into()),
            Some(ClauseGate::Failed) | None => reject("guardian-keys state unavailable".into()),
        }
    }

    /// A handle on the port for a PIN check or a mint run outside the
    /// warden lock, with this machine's pubkey for the presence code.
    pub fn presence_handle(&self) -> Option<(GuardianKeys, PubKey)> {
        self.guardian_keys
            .clone()
            .map(|k| (k, self.signer.pubkey()))
    }

    /// Whether a guardian has set the ward's PIN.
    pub fn has_guardian_pin(&self) -> bool {
        self.guardian_keys
            .as_ref()
            .is_some_and(|k| k.has_guardian_pin())
    }

    /// The guardian-keys document as stored (labels and the PIN counter are
    /// not on the port's API). Read only; `None` when absent or unreadable.
    fn raw_state(&self) -> Option<Value> {
        FileGuardianStateStore::in_dir(&self.base)
            .load()
            .ok()
            .flatten()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    }

    /// The pending co-guardian, removal and move statements, for the ward's
    /// Guardian presence screen. A JSON list of `{d, kind, signer, key,
    /// label, issuedAt, expiresAt, takesEffectAt, confirmed}`: `signer` is
    /// the root that signed it, `key` the root it names (R2).
    pub fn pending_statements_json(&self, now: u64) -> String {
        let Some(keys) = &self.guardian_keys else {
            return "[]".into();
        };
        let raw = self.raw_state();
        let list: Vec<Value> = keys
            .pending_statements(now)
            .into_iter()
            .map(|p| {
                let key = p.d.rsplit('/').next().unwrap_or_default().to_owned();
                json!({
                    "d": p.d,
                    "kind": serde_json::to_value(p.kind).unwrap_or(Value::Null),
                    "signer": p.signer,
                    "key": key,
                    "label": statement_label(raw.as_ref(), &p.d),
                    "issuedAt": p.issued_at,
                    "expiresAt": p.expires_at,
                    "takesEffectAt": p.takes_effect_at,
                    "confirmed": p.confirmed_at.is_some(),
                })
            })
            .collect();
        Value::Array(list).to_string()
    }

    /// The guardian list for About (and rule 5's "who set a rule"): each
    /// pinned root (hex; Kotlin shows it as a short npub beside its label),
    /// its active working keys, the rule kinds it set, plus the pending count
    /// and the PIN's state (set, failures, seconds until the next try).
    pub fn guardians_summary_json(&self, now: u64) -> String {
        let Some(keys) = &self.guardian_keys else {
            return json!({"available": false, "paired": false, "roots": [], "pending": 0,
                "pin": {"set": false, "failures": 0, "waitSeconds": 0}})
            .to_string();
        };
        let Some(projected) = keys.project(now) else {
            return json!({"available": false, "paired": true, "roots": [], "pending": 0,
                "pin": {"set": false, "failures": 0, "waitSeconds": 0}})
            .to_string();
        };
        let raw = self.raw_state();
        let roots: Vec<Value> = projected
            .roots
            .iter()
            .map(|r| {
                let key_labels: Vec<Value> = r
                    .keys
                    .iter()
                    .filter_map(|k| PubKey::from_hex(k).ok())
                    .filter_map(|k| keys.signer_label(&k).map(Value::String))
                    .collect();
                let mut rules: Vec<String> = projected
                    .clauses
                    .iter()
                    .filter(|c| c.root == r.root)
                    .map(|c| c.kind.clone())
                    .collect();
                rules.sort();
                rules.dedup();
                json!({
                    "root": r.root,
                    "label": statement_label(raw.as_ref(), &format!("kintrinsic/guardian/{}", r.root)),
                    "keys": r.keys.len(),
                    "keyLabels": key_labels,
                    "rules": rules,
                })
            })
            .collect();
        let (failures, last) = raw
            .as_ref()
            .and_then(|r| r.get("pinAttempts"))
            .map(|a| {
                (
                    a.get("failures").and_then(Value::as_u64).unwrap_or(0),
                    a.get("lastFailureAt").and_then(Value::as_i64).unwrap_or(0),
                )
            })
            .unwrap_or((0, 0));
        json!({
            "available": true,
            "paired": true,
            "roots": roots,
            "pending": projected.pending.len(),
            "pin": {
                "set": projected.guardian_pin.is_some(),
                "failures": failures,
                "waitSeconds": pin_wait(failures, last, now),
            },
        })
        .to_string()
    }
}

/// Seconds until the next PIN try is checked (contract: after 5 failures the
/// next try waits 1 minute, doubling to a 1-hour cap).
fn pin_wait(failures: u64, last_failure_at: i64, now: u64) -> i64 {
    if failures < charter_guardians::pin::PIN_FREE_FAILURES {
        return 0;
    }
    let next = charter_guardians::pin_next_try_at(failures, last_failure_at);
    (next - now as i64).max(0)
}

/// The label a stored statement with `d` carried, if any.
fn statement_label(raw: Option<&Value>, d: &str) -> Option<String> {
    raw?.get("state")?
        .get("statements")?
        .as_array()?
        .iter()
        .find(|s| s.get("d").and_then(Value::as_str) == Some(d))
        .and_then(|s| s.get("label").and_then(Value::as_str))
        .map(str::to_owned)
}

/// A PIN check's answer for Kotlin: `{ok, failures, waitSeconds}` plus
/// `noPin` / `unavailable` when nothing was checked.
fn pin_json(check: PinCheck, now: u64) -> Value {
    let wait = |until: i64| (until - now as i64).max(0);
    match check {
        PinCheck::Correct => json!({"ok": true, "failures": 0, "waitSeconds": 0}),
        PinCheck::Wrong {
            failures,
            next_try_at,
        } => json!({"ok": false, "failures": failures, "waitSeconds": wait(next_try_at)}),
        PinCheck::Wait { until } => {
            json!({"ok": false, "waitSeconds": wait(until), "error": "wait"})
        }
        PinCheck::NoPin => json!({"ok": false, "noPin": true, "waitSeconds": 0, "error": "noPin"}),
        PinCheck::Unavailable => {
            json!({"ok": false, "unavailable": true, "waitSeconds": 0, "error": "unavailable"})
        }
    }
}

/// `verifyGuardianPin`: check an entered PIN (the attempt is counted and
/// written before scrypt runs; a correct one resets the counter).
pub fn pin_check_json(keys: Option<&GuardianKeys>, pin: &str, now: u64) -> String {
    match keys {
        None => pin_json(PinCheck::Unavailable, now).to_string(),
        Some(k) => pin_json(k.verify_guardian_pin(pin, now), now).to_string(),
    }
}

/// Percent-encode a query value (RFC 3986 unreserved characters kept).
fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The presence code (open question "Presence code encoding"), mirroring the
/// scan-to-pair invite: `charter://presence?m=<machine>&t=<token>&d=<d>`.
pub fn presence_code(machine: &PubKey, token: &str, d: Option<&str>) -> String {
    let mut code = format!("charter://presence?m={}&t={token}", machine.to_hex());
    if let Some(d) = d {
        code.push_str("&d=");
        code.push_str(&pct(d));
    }
    code
}

/// The short form to type: the token as four groups of eight.
pub fn short_form(token: &str) -> String {
    token
        .as_bytes()
        .chunks(8)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The QR module grid for `text`, row-major, `1` dark and `0` light.
fn qr_json(text: &str) -> Value {
    match qrcode::QrCode::new(text.as_bytes()) {
        Ok(qr) => {
            let modules: String = qr
                .to_colors()
                .into_iter()
                .map(|c| if c == qrcode::Color::Dark { '1' } else { '0' })
                .collect();
            json!({"size": qr.width(), "modules": modules})
        }
        Err(_) => Value::Null,
    }
}

/// `mintPresenceToken`: a presence token for the pending statement `d`
/// (`None`: any), only with a correct guardian PIN in the same call and
/// never during a backoff. Success is `{code, shortForm, expiresAt, qr}`;
/// otherwise `{error, …}` with the PIN answer's fields (`noPin`, `wrongPin`,
/// `wait`, `notPending`, `unavailable`, `failed`). The ward's own screen
/// never accepts a tap as confirmation: the token is only shown, for a
/// guardian install bound to the right root to scan.
pub fn mint_presence_json(
    keys: Option<&GuardianKeys>,
    machine: &PubKey,
    d: Option<&str>,
    pin: &str,
    now: u64,
) -> String {
    let Some(keys) = keys else {
        return json!({"error": "unavailable"}).to_string();
    };
    let mut fill = |buf: &mut [u8]| {
        getrandom::getrandom(buf).expect("OS entropy unavailable: refusing a predictable token")
    };
    match keys.mint_presence_token_with_pin(pin, d, now, &mut fill) {
        Ok(t) => {
            let code = presence_code(machine, &t.token, t.bound_to.as_deref());
            json!({
                "code": code,
                "shortForm": short_form(&t.token),
                "expiresAt": t.expires_at,
                "qr": qr_json(&code),
            })
            .to_string()
        }
        Err(PresenceError::Pin(check)) => {
            let mut v = pin_json(check, now);
            if matches!(check, PinCheck::Wrong { .. }) {
                v["error"] = json!("wrongPin");
            }
            v.to_string()
        }
        Err(PresenceError::NotPending) => json!({"error": "notPending"}).to_string(),
        Err(PresenceError::Unavailable) => json!({"error": "unavailable"}).to_string(),
        Err(PresenceError::Failed) => json!({"error": "failed"}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! The Android ward with guardian roots and working keys, end to end
    //! over the warden's own broker (the local-link ingest path) and its
    //! direct clause ingest. Fixtures follow
    //! `charter-spine/tests/guardian_keys.rs`. The test guardian PIN is
    //! always 123456.

    use super::*;
    use charter_link::AckResult;
    use charter_primitives::kinds;
    use charter_spine::LocalIngestOptions;
    use charter_sys::relay::MockRelayTransport;
    use charter_transport::nip59::{self, Rumor, WrapRandomness};
    use charter_verify::test_support::{sign_event, ClauseBuilder, TestGuardian};
    use std::path::PathBuf;

    const ROOT: u8 = 0x11;
    const W1: u8 = 0x12;
    const ROOT2: u8 = 0x13;
    const W2: u8 = 0x14;
    const STRANGER: u8 = 0x33;
    const PIN: &str = "123456";

    fn secret(b: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = b;
        s
    }

    fn pk(seed: u8) -> PubKey {
        PubKey::from_bytes(charter_crypto::xonly_pubkey(&secret(seed)).unwrap())
    }

    fn temp_base(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "charter-gk-{label}-{}-{}",
            std::process::id(),
            wall_now()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A ward paired by its `bunker://` link to ROOT (1000 s ago), with both
    /// relay halves and the broker over an in-memory relay.
    struct Ward {
        base: PathBuf,
        w: Warden,
        mock: MockRelayTransport,
        now: u64,
    }

    impl Ward {
        fn new(tag: &str) -> Ward {
            let base = temp_base(tag);
            let now = wall_now();
            let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
            let uri = format!(
                "bunker://{}?relay=wss://relay.example&kind=charter",
                pk(ROOT).to_hex()
            );
            w.pair(&uri, now - 1000).unwrap();
            let mock = MockRelayTransport::new();
            w.wire_test_relay(mock.clone()).unwrap();
            Ward { base, w, mock, now }
        }

        fn machine(&self) -> PubKey {
            self.w.signer.pubkey()
        }

        fn subject(&self) -> PubKey {
            self.w.subject.unwrap()
        }

        /// `ev` sealed by the key with low byte `sealer`, wrapped to the ward.
        fn sealed(&self, ev: &NostrEvent, sealer: u8, salt: u8) -> NostrEvent {
            let r = WrapRandomness {
                ephemeral_secret: secret(salt | 0x40),
                seal_nonce: [salt; 32],
                wrap_nonce: [salt.wrapping_add(1); 32],
                seal_created_at: self.now,
                wrap_created_at: self.now,
            };
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &secret(sealer),
                self.machine().as_bytes(),
                &r,
            )
            .unwrap()
        }

        /// What the ward's broker made of `wrap` arriving over the link.
        fn ingest(&self, wrap: &NostrEvent) -> AckResult {
            let b = self.w.broker.clone().expect("a broker");
            self.w
                .rt
                .block_on(b.ingest_local(
                    wrap,
                    LocalIngestOptions {
                        handle_release: false,
                    },
                ))
                .result
        }

        fn deliver(&self, ev: &NostrEvent, sealer: u8, salt: u8) -> AckResult {
            self.ingest(&self.sealed(ev, sealer, salt))
        }

        fn clause(&self, seed: u8, issued_at: u64, minutes: u32) -> NostrEvent {
            ClauseBuilder::budget(issued_at)
                .subject(self.subject())
                .body(json!({"dailyMinutes": minutes, "tz": "UTC"}))
                .build(&TestGuardian::from_seed(seed))
        }

        fn held_minutes(&self) -> Option<u64> {
            self.w
                .child_clauses
                .get_child_clause(&self.subject().to_hex(), ClauseKind::Budget.store_key())
                .unwrap()
                .and_then(|j| serde_json::from_str::<Value>(&j).ok())
                .and_then(|v| v["dailyMinutes"].as_u64())
        }

        fn statement(
            &self,
            seed: u8,
            d: String,
            p: PubKey,
            issued_at: u64,
            extra: Value,
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

        fn approval(&self, root: u8, w: u8) -> NostrEvent {
            self.statement(
                root,
                format!("kintrinsic/device/{}", pk(w).to_hex()),
                pk(w),
                self.now - 500,
                json!({"label": format!("phone {w:#x}")}),
            )
        }

        /// ROOT approves W1, sealed by W1 (proof of possession).
        fn approve_w1(&self) {
            assert_eq!(
                self.deliver(&self.approval(ROOT, W1), W1, 1),
                AckResult::Applied
            );
        }

        fn relays_event(&self, seed: u8, at: u64, pin: bool) -> NostrEvent {
            let mut body = json!({
                "machine": self.machine().to_hex(),
                "relays": ["wss://relay.example", "wss://relay2.example"],
            });
            if pin {
                body["guardianPin"] = pin_record();
            }
            sign_event(
                &TestGuardian::from_seed(seed).signer,
                kinds::CHARTER_DEVICE_RELAYS,
                at,
                vec![
                    kinds::marker_tag(),
                    vec!["p".into(), self.machine().to_hex()],
                ],
                body.to_string(),
            )
        }

        /// ROOT sets the guardian PIN 123456 in a gift-wrapped RELAYS.
        fn set_pin(&self) {
            let ev = self.relays_event(ROOT, self.now - 10, true);
            assert_eq!(self.deliver(&ev, ROOT, 7), AckResult::Applied);
        }

        fn handle(&self) -> (GuardianKeys, PubKey) {
            self.w.presence_handle().expect("guardian keys loaded")
        }

        /// The ward again, as after a restart (a new process over the same
        /// Device Owner base dir).
        fn restarted(self) -> Ward {
            let base = self.base.clone();
            let (mock, now) = (self.mock.clone(), self.now);
            drop(self.w);
            let w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
            Ward { base, w, mock, now }
        }
    }

    fn pin_record() -> Value {
        let salt = [0x5a; 16];
        let hash = charter_guardians::pin_hash(PIN, &salt);
        json!({
            "v": 1,
            "salt": "5a".repeat(16),
            "hash": hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        })
    }

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    /// The recipient a gift wrap names.
    fn wrap_p(ev: &NostrEvent) -> Option<String> {
        ev.tags
            .iter()
            .find(|t| t.first().is_some_and(|n| n == "p"))
            .and_then(|t| t.get(1).cloned())
    }

    /// Every gift wrap on the in-memory relay.
    fn relay_wraps(mock: &MockRelayTransport) -> Vec<NostrEvent> {
        use charter_sys::relay::{Filter, RelayTransport};
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(mock.query(
            &["wss://relay.example".to_string()],
            Filter {
                kinds: vec![kinds::GIFT_WRAP],
                ..Default::default()
            },
        ))
        .unwrap()
    }

    /// ROOT names ROOT2 a co-guardian (sealed by W1, exception 2); the ward
    /// mints a token for it with the guardian PIN; W2, approved by ROOT2,
    /// confirms in person. Returns the co-guardian statement.
    fn add_coguardian(ward: &Ward) -> NostrEvent {
        ward.approve_w1();
        ward.set_pin();
        let cg = ward.statement(
            ROOT,
            format!("kintrinsic/guardian/{}", pk(ROOT2).to_hex()),
            pk(ROOT2),
            ward.now - 50,
            json!({"label": "Other parent"}),
        );
        assert_eq!(ward.deliver(&cg, W1, 2), AckResult::Applied);
        let pending = parse(&ward.w.pending_statements_json(ward.now));
        assert_eq!(pending.as_array().unwrap().len(), 1);
        assert_eq!(pending[0]["kind"], "coguardian");
        assert_eq!(pending[0]["key"], pk(ROOT2).to_hex());
        assert_eq!(pending[0]["signer"], pk(ROOT).to_hex());
        assert_eq!(pending[0]["label"], "Other parent");
        let d = pending[0]["d"].as_str().unwrap().to_owned();

        let (keys, machine) = ward.handle();
        let minted = parse(&mint_presence_json(
            Some(&keys),
            &machine,
            Some(&d),
            PIN,
            ward.now,
        ));
        let code = minted["code"].as_str().expect("a code").to_owned();
        assert!(code.starts_with(&format!("charter://presence?m={}&t=", machine.to_hex())));
        assert!(code.ends_with(&format!(
            "&d=kintrinsic%2Fguardian%2F{}",
            pk(ROOT2).to_hex()
        )));
        assert_eq!(minted["expiresAt"], ward.now + 600);
        let token = code
            .split("&t=")
            .nth(1)
            .and_then(|r| r.split('&').next())
            .unwrap()
            .to_owned();
        assert_eq!(token.len(), 32);
        assert_eq!(minted["shortForm"], short_form(&token));
        let qr = &minted["qr"];
        let size = qr["size"].as_u64().unwrap() as usize;
        assert_eq!(qr["modules"].as_str().unwrap().len(), size * size);

        let w2_approval = ward.approval(ROOT2, W2);
        let confirm = sign_event(
            &TestGuardian::from_seed(W2).signer,
            kinds::CHARTER_PRESENCE_CONFIRM,
            ward.now,
            vec![kinds::marker_tag()],
            json!({"v": 1, "token": token, "statement": cg.id.to_hex(),
                   "approval": w2_approval})
            .to_string(),
        );
        assert_eq!(ward.deliver(&confirm, W2, 3), AckResult::Applied);
        cg
    }

    #[test]
    fn a_working_keys_clause_is_accepted_only_once_approved() {
        let ward = Ward::new("approve");
        // Not in the signer set yet: dropped, not buffered.
        let early = ward.clause(W1, ward.now - 30, 40);
        assert_eq!(ward.deliver(&early, W1, 9), AckResult::Refused);
        assert_eq!(ward.held_minutes(), None);

        ward.approve_w1();
        assert_eq!(ward.deliver(&early, W1, 10), AckResult::Applied);
        assert_eq!(ward.held_minutes(), Some(40));
        // The root still signs directly too.
        let by_root = ward.clause(ROOT, ward.now - 20, 45);
        assert_eq!(ward.deliver(&by_root, ROOT, 11), AckResult::Applied);
        assert_eq!(ward.held_minutes(), Some(45));
        // The direct ingest takes the same gate: a stranger is refused, W1 is
        // not, and the clause records its signer (rule 5).
        let mut ward = ward;
        let stranger = ward.clause(STRANGER, ward.now - 10, 99);
        let r = ward
            .w
            .ingest_clause(&serde_json::to_string(&stranger).unwrap(), ward.now);
        assert!(!r.accepted, "{}", r.reason);
        let by_w1 = ward.clause(W1, ward.now - 5, 50);
        let r = ward
            .w
            .ingest_clause(&serde_json::to_string(&by_w1).unwrap(), ward.now);
        assert!(r.accepted, "{}", r.reason);
        assert_eq!(ward.held_minutes(), Some(50));
        let summary = parse(&ward.w.guardians_summary_json(ward.now));
        assert_eq!(summary["roots"][0]["root"], pk(ROOT).to_hex());
        assert_eq!(summary["roots"][0]["keys"], 1);
        assert_eq!(summary["roots"][0]["keyLabels"][0], "phone 0x12");
        assert_eq!(summary["roots"][0]["rules"], json!(["budget"]));
    }

    #[test]
    fn a_withdrawn_working_key_is_refused_for_good() {
        let mut ward = Ward::new("withdraw");
        ward.approve_w1();
        assert_eq!(
            ward.deliver(&ward.clause(W1, ward.now - 40, 30), W1, 2),
            AckResult::Applied
        );
        let withdrawal = ward.statement(
            ROOT,
            format!("kintrinsic/revoke/{}", pk(W1).to_hex()),
            pk(W1),
            ward.now - 30,
            json!({}),
        );
        assert_eq!(ward.deliver(&withdrawal, ROOT, 3), AckResult::Applied);
        // Rule 4: its clause stays enforced, but nothing new from it lands.
        assert_eq!(ward.held_minutes(), Some(30));
        assert_eq!(
            ward.deliver(&ward.clause(W1, ward.now - 20, 90), W1, 4),
            AckResult::Refused
        );
        let direct = ward.clause(W1, ward.now - 10, 91);
        let r = ward
            .w
            .ingest_clause(&serde_json::to_string(&direct).unwrap(), ward.now);
        assert!(!r.accepted);
        // No approval revives it.
        assert_ne!(
            ward.deliver(&ward.approval(ROOT, W1), W1, 5),
            AckResult::Applied
        );
        // A current signer replaces the departed key's rule.
        assert_eq!(
            ward.deliver(&ward.clause(ROOT, ward.now - 5, 35), ROOT, 6),
            AckResult::Applied
        );
        assert_eq!(ward.held_minutes(), Some(35));
    }

    #[test]
    fn a_two_root_family_reaches_both_parents_and_refuses_a_remote_release() {
        let ward = Ward::new("tworoot");
        add_coguardian(&ward);
        let summary = parse(&ward.w.guardians_summary_json(ward.now));
        let roots: Vec<&str> = summary["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["root"].as_str().unwrap())
            .collect();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&pk(ROOT2).to_hex().as_str()));
        assert_eq!(summary["pending"], 0);
        // W2 (the other parent) sets a rule, equal authority.
        assert_eq!(
            ward.deliver(&ward.clause(W2, ward.now - 1, 20), W2, 8),
            AckResult::Applied
        );
        assert_eq!(ward.held_minutes(), Some(20));

        // AUDIT (break-glass) goes to both working keys, one wrap each.
        let relay = ward.w.relay.clone().unwrap();
        let audits = relay.build_audit_wraps(crate::breakglass::audit_tags("full", 60), ward.now);
        let mut to: Vec<String> = audits.iter().filter_map(wrap_p).collect();
        to.sort();
        let mut want = vec![pk(W1).to_hex(), pk(W2).to_hex()];
        want.sort();
        assert_eq!(to, want);

        // STATUS carries `guardians` and appVersionCode, to both, one wrap each.
        let status = ward.w.build_status(ward.now).unwrap();
        assert_eq!(status.guardians.as_ref().unwrap().roots.len(), 2);
        assert_eq!(status.app_version_code, Some(20));
        assert!(relay_wraps(&ward.mock).is_empty());
        let Ward { w, mock, now, .. } = ward;
        let lock = std::sync::Mutex::new(Some(w));
        let r = super::super::poll_once_locked(&lock, now);
        assert!(r.status_emitted, "{}", r.reason);
        let mut status_to: Vec<String> = relay_wraps(&mock).iter().filter_map(wrap_p).collect();
        status_to.sort();
        assert_eq!(status_to, want, "one STATUS wrap per working key");

        // A remote RELEASE from either root is refused while two are pinned.
        let mut g = lock.lock().unwrap();
        let w = g.as_mut().unwrap();
        let release = sign_event(
            &TestGuardian::from_seed(ROOT).signer,
            kinds::CHARTER_DEVICE_RELEASE,
            now,
            vec![kinds::marker_tag()],
            json!({"v": 1, "machine": w.signer.pubkey().to_hex(), "issuedAt": now}).to_string(),
        );
        let (applied, detail) = w.try_apply_releases(&[(release, pk(ROOT))], now);
        assert!(!applied);
        assert!(detail.contains("MoreThanOneRoot"), "{detail}");
        assert!(w.guardian.is_some());
    }

    #[test]
    fn a_release_from_the_one_root_unpairs_but_a_forged_seal_does_not() {
        let mut ward = Ward::new("release");
        let machine = ward.machine().to_hex();
        let release = |seed: u8| {
            sign_event(
                &TestGuardian::from_seed(seed).signer,
                kinds::CHARTER_DEVICE_RELEASE,
                ward.now,
                vec![kinds::marker_tag()],
                json!({"v": 1, "machine": machine, "issuedAt": ward.now}).to_string(),
            )
        };
        // L3: the seal must be the author's.
        let (applied, detail) = ward
            .w
            .try_apply_releases(&[(release(ROOT), pk(STRANGER))], ward.now);
        assert!(!applied, "{detail}");
        assert!(detail.contains("SealMismatch"), "{detail}");
        let (applied, _) = ward
            .w
            .try_apply_releases(&[(release(STRANGER), pk(STRANGER))], ward.now);
        assert!(!applied);
        let (applied, _) = ward
            .w
            .try_apply_releases(&[(release(ROOT), pk(ROOT))], ward.now);
        assert!(applied);
        assert!(ward.w.guardian_keys.is_none());
        assert!(!ward.base.join("guardian-state.json").exists());
    }

    #[test]
    fn the_pin_backs_off_and_its_failures_survive_a_restart() {
        let ward = Ward::new("pin");
        let (keys, _) = ward.handle();
        assert_eq!(
            parse(&pin_check_json(Some(&keys), PIN, ward.now))["noPin"],
            true
        );
        ward.set_pin();
        assert!(ward.w.has_guardian_pin());
        // L2: the PIN record never reaches the pairing record.
        let pairing = ward.w.pairing.load().unwrap().unwrap();
        assert!(!pairing.contains(&"5a".repeat(16)));
        assert!(!pairing.contains("guardianPin") && !pairing.contains("guardian_pin"));
        for n in 1..=4 {
            let v = parse(&pin_check_json(Some(&keys), "000000", ward.now));
            assert_eq!(v["ok"], false);
            assert_eq!(v["failures"], n);
            assert_eq!(v["waitSeconds"], 0);
        }
        let v = parse(&pin_check_json(Some(&keys), "000000", ward.now));
        assert_eq!(v["failures"], 5);
        assert_eq!(v["waitSeconds"], 60);

        // A restart keeps the count: the right PIN still waits.
        let ward = ward.restarted();
        let summary = parse(&ward.w.guardians_summary_json(ward.now + 1));
        assert_eq!(summary["pin"]["set"], true);
        assert_eq!(summary["pin"]["failures"], 5);
        assert_eq!(summary["pin"]["waitSeconds"], 59);
        let (keys, _) = ward.handle();
        let v = parse(&pin_check_json(Some(&keys), PIN, ward.now + 1));
        assert_eq!(v["error"], "wait");
        assert_eq!(v["waitSeconds"], 59);
        let v = parse(&pin_check_json(Some(&keys), PIN, ward.now + 60));
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn no_presence_token_without_the_right_pin() {
        let ward = Ward::new("mint");
        let (keys, machine) = ward.handle();
        let v = parse(&mint_presence_json(
            Some(&keys),
            &machine,
            None,
            PIN,
            ward.now,
        ));
        assert_eq!(v["error"], "noPin");
        assert!(v.get("code").is_none());
        ward.set_pin();
        let v = parse(&mint_presence_json(
            Some(&keys),
            &machine,
            None,
            "654321",
            ward.now,
        ));
        assert_eq!(v["error"], "wrongPin");
        assert_eq!(v["failures"], 1);
        assert!(v.get("code").is_none());
        // The right PIN for something that is not pending: no token.
        let v = parse(&mint_presence_json(
            Some(&keys),
            &machine,
            Some("kintrinsic/guardian/00"),
            PIN,
            ward.now,
        ));
        assert_eq!(v["error"], "notPending");
        // Backing off: even the right PIN is not checked.
        for _ in 0..5 {
            let _ = pin_check_json(Some(&keys), "000000", ward.now);
        }
        let v = parse(&mint_presence_json(
            Some(&keys),
            &machine,
            None,
            PIN,
            ward.now,
        ));
        assert_eq!(v["error"], "wait");
        assert!(v.get("code").is_none());
    }

    #[test]
    fn a_pasted_relay_code_may_not_carry_a_pin_but_any_signer_may_sign_one() {
        let mut ward = Ward::new("pasted");
        let with_pin = ward.relays_event(ROOT, ward.now - 5, true);
        let err = ward
            .w
            .update_relays(&serde_json::to_string(&with_pin).unwrap(), ward.now)
            .unwrap_err();
        assert!(err.contains("PIN"), "{err}");
        assert!(!ward.w.has_guardian_pin());
        assert_eq!(ward.w.relays, vec!["wss://relay.example".to_string()]);
        // A stranger's code is refused; an approved working key's applies.
        let stranger = ward.relays_event(STRANGER, ward.now - 4, false);
        assert!(ward
            .w
            .update_relays(&serde_json::to_string(&stranger).unwrap(), ward.now)
            .is_err());
        ward.approve_w1();
        let by_w1 = ward.relays_event(W1, ward.now - 3, false);
        let state = ward
            .w
            .update_relays(&serde_json::to_string(&by_w1).unwrap(), ward.now)
            .unwrap();
        assert_eq!(state.relays.len(), 2);
        // The guardian-keys state followed the pairing's list.
        let g = ward.w.guardian_keys.as_ref().unwrap();
        assert_eq!(g.status_guardians(ward.now).unwrap().relays.len(), 2);
    }

    #[test]
    fn a_ward_paired_before_the_section_keeps_its_clauses_enforced() {
        let base = temp_base("migrate");
        let now = wall_now();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            pk(ROOT).to_hex()
        );
        w.pair(&uri, now - 1000).unwrap();
        let subject = w.subject.unwrap().to_hex();
        // A paused schedule stored the way a build before the section stored
        // it: no signer recorded, and no guardian-keys state on disk.
        let body = json!({"v": 1, "tz": "UTC", "paused": true, "weekly": {},
                          "issuedAt": now - 500});
        w.child_clauses
            .put_child_clause(
                &subject,
                ClauseKind::Schedule.store_key(),
                now - 500,
                &body.to_string(),
            )
            .unwrap();
        drop(w);
        std::fs::remove_file(base.join("guardian-state.json")).unwrap();

        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        assert!(
            base.join("guardian-state.json").exists(),
            "migrated in place"
        );
        let summary = parse(&w.guardians_summary_json(now));
        assert_eq!(summary["roots"][0]["root"], pk(ROOT).to_hex());
        let d = w.tick(Some(&subject), true, now as i64, None);
        assert!(
            d.iter().any(|c| c.locked),
            "the migrated ward still enforces its clause"
        );
        // Its replay floor carried over: an older clause is refused, a newer
        // one from the root is accepted.
        let older = ClauseBuilder::schedule(now - 600)
            .body(json!({"v": 1, "tz": "UTC", "paused": false, "weekly": {},
                         "issuedAt": now - 600}))
            .build(&TestGuardian::from_seed(ROOT));
        let r = w.ingest_clause(&serde_json::to_string(&older).unwrap(), now);
        assert!(!r.accepted, "{}", r.reason);
        let newer = ClauseBuilder::schedule(now - 100)
            .body(json!({"v": 1, "tz": "UTC", "paused": false, "weekly": {},
                         "issuedAt": now - 100}))
            .build(&TestGuardian::from_seed(ROOT));
        let r = w.ingest_clause(&serde_json::to_string(&newer).unwrap(), now);
        assert!(r.accepted, "{}", r.reason);
    }

    #[test]
    fn the_presence_code_and_short_form() {
        let m = pk(0x22);
        assert_eq!(
            presence_code(&m, &"ab".repeat(16), Some("kintrinsic/move/x y")),
            format!(
                "charter://presence?m={}&t={}&d=kintrinsic%2Fmove%2Fx%20y",
                m.to_hex(),
                "ab".repeat(16)
            )
        );
        assert_eq!(
            short_form("0123456789abcdef0123456789abcdef"),
            "01234567 89abcdef 01234567 89abcdef"
        );
    }
}
