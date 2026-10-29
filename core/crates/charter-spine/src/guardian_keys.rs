//! The guardian-keys adapter (contract, *Guardian roots and working keys
//! (v1)*): the [`GuardianStatements`] port and the live [`SignerSet`] a ward
//! that implements the section plugs into its broker, backed by the
//! vector-tested `charter-guardians` state machine and persisted through a
//! [`GuardianStateStore`] (`0600`, atomic).
//!
//! # Platform API
//!
//! - [`GuardianKeys::load`]: build the adapter for a paired ward from its
//!   pairing record. A ward paired before the section (no state file) is
//!   migrated in place: its state is initialised as if paired to the pinned
//!   guardian as its one root.
//! - [`GuardianKeys::from_paired`]: after a scan-to-pair ([`scan_to_pair`]),
//!   persist the new state and build the adapter.
//! - `Broker::with_guardians(Arc::new(keys.clone()))`, and
//!   [`GuardianStatements::signers`] for `CharterTransport::set_signer_set`
//!   and the local link.
//! - [`release_gate`]: the RELEASE decision both platforms make (seal ==
//!   author, signer set, more than one root refused, pairing floor).
//! - Presence: [`GuardianKeys::pending_statements`] to show,
//!   [`GuardianKeys::mint_presence_token`] (Linux, after polkit) or
//!   [`GuardianKeys::mint_presence_token_with_pin`] (Android), and the
//!   confirmation arrives as a kind-31121 wrap through the broker.
//! - The Android guardian PIN: [`GuardianKeys::verify_guardian_pin`].
//! - STATUS: [`GuardianStatements::status_guardians`] and
//!   [`GuardianStatements::status_relays`] for the STATUS every recipient
//!   in the signer set gets; [`GuardianKeys::farewell`] for the departed
//!   keys owed one last STATUS (a separate, minimal body, sent to each
//!   separately) and [`GuardianKeys::mark_status_sent`] with the keys it
//!   reached.
//!
//! # Persistence (rule 10)
//!
//! Every change is computed as a `charter-guardians` step on a copy, the
//! resulting document is written, and only then is it swapped in, all under
//! one lock: the live signer set never answers from a state that is not on
//! disk. A write failure leaves the old state in force and the change
//! unapplied (`Failed`). The one exception is the timer advance
//! ([`GuardianStatements::advance`]): it is a pure function of the persisted
//! state and the clock, so it is applied even when its write fails, and is
//! re-derived after a restart.
//!
//! # Invariant: once a ward has a state document, every broker carries the port
//!
//! A broker built without this port judges per-child clauses by the clause
//! store's replay floor alone, so the store can get ahead of the state; the
//! next clause the port judges between the two floors would then be accepted
//! and written BELOW a newer body (the review-2a M1 rollback). A platform must
//! never build the broker without the port for a ward that has one (no
//! conditional wiring, no recovery mode without it).
//!
//! # The pairing record's `guardian_pubkey` after a root move
//!
//! The state document is authoritative for who the ward trusts. The pairing
//! record's `guardian_pubkey` stays the root pinned at pairing, even after a
//! confirmed root move replaces it: it is read only as the signer of a clause
//! record that names none (a clause stored before the section, rule 5), and
//! as the transport's legacy single recipient when no signer set is wired.
//! Rewriting it to the new root would attribute every such clause to a
//! CURRENT signer, so its departure floor (rule 4) would no longer apply, and
//! it would be one more read-modify-write of a record another process also
//! writes (review 2a, Info 2). Nothing that decides authority reads it on a
//! ward with this adapter: the signer set, recipients, the local link and the
//! offline unlock all come from the state.
//!
//! # Fail closed
//!
//! A state document that exists but cannot be read or parsed is NOT replaced
//! by a migration: the adapter is then unavailable. Its signer set is empty
//! (no CLAUSE, GRANT, RELAYS, USAGE_SYNC, RELEASE or statement is accepted,
//! no local-link handshake completes), it has no recipients (nothing is
//! encrypted to a key that may have departed), and every port call fails.
//! The clauses already stored stay enforced. The way out is a local unpair
//! by an administrator, or restoring the file. Each timer advance retries
//! the read, so a transient read error heals itself.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use charter_guardians::{self as cg, GuardianState, Reason, Verdict};
use charter_link::AckResult;
use charter_primitives::{kinds, NostrEvent, PubKey, SignerSet};
use charter_proto::{
    PendingKind, ReleasePayload, StatusGuardianRoot, StatusGuardians, StatusPendingStatement,
};
use charter_sys::guardian_store::GuardianStateStore;
use charter_sys::presence_token::PresenceTokenStore;
use charter_sys::{SysError, SysResult};
use charter_transport::{GuardianSealed, Pairing, ReceivedPairOffer};
use serde::{Deserialize, Serialize};

use crate::ports::{
    ClauseCandidate, ClauseGate, GuardianApplied, GuardianStatements, RelaysChange, RelaysRecord,
    SlotSeed,
};

/// The document's own version.
const DOC_VERSION: u8 = 1;

/// The persisted guardian-PIN failure counter (contract: "The counter
/// survives restarts").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinAttempts {
    /// Consecutive wrong entries (an entry is counted before it is checked).
    pub failures: u64,
    /// Ward time of the last counted entry.
    pub last_failure_at: i64,
}

/// What the store holds: the vector-tested state, plus what the ward keeps
/// beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Doc {
    v: u8,
    state: GuardianState,
    #[serde(default)]
    pin_attempts: PinAttempts,
    /// The guardian the pairing record pinned when this document was made
    /// (hex), so a document is matched to its pairing by more than
    /// `pairedAt` (review batch 2, core L4). Absent in a document written
    /// before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paired_guardian: Option<String>,
}

/// Whose a stored document is, against the pairing record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocPairing {
    /// This pairing's.
    Same,
    /// An earlier pairing's: the record was replaced by a newer pairing (an
    /// administrator's re-pair, which clears the state; a stale write by the
    /// warden it replaced can still land after the clear).
    Earlier,
    /// Neither: same `pairedAt` with another guardian, or a document newer
    /// than the record. Nothing legitimate writes that; fail closed.
    Other,
}

fn doc_pairing(doc: &Doc, paired_at: i64, guardian: &str) -> DocPairing {
    let same_guardian = doc.paired_guardian.as_deref().is_none_or(|g| g == guardian);
    if doc.state.paired_at == paired_at && same_guardian {
        DocPairing::Same
    } else if doc.state.paired_at < paired_at {
        DocPairing::Earlier
    } else {
        DocPairing::Other
    }
}

/// One guardian-PIN check at a time in the process: the scrypt (32 MiB) runs
/// once per entry, and entries are judged one after another.
static PIN_CHECK: Mutex<()> = Mutex::new(());

// One per adapter, behind a mutex: the size difference costs nothing.
#[allow(clippy::large_enum_variant)]
enum Held {
    Ready(Doc),
    /// Unreadable, or cleared by a release. See the module docs.
    Unavailable(&'static str),
}

struct Inner {
    store: Arc<dyn GuardianStateStore>,
    /// The pairing this adapter serves (`paired_at`, and the pinned
    /// guardian's hex): a document read back later (a read error healing)
    /// must belong to it.
    paired_at: i64,
    paired_guardian: String,
    held: Mutex<Held>,
    /// The outcome `persist` computed, for `apply` to report.
    staged: Mutex<Option<(charter_primitives::EventId, GuardianApplied)>>,
    /// Where the current presence token is also shown from, if anywhere.
    token_file: Mutex<Option<PresenceTokenStore>>,
}

/// The guardian-keys port for one paired ward. Cheap to clone; clones share
/// one state.
#[derive(Clone)]
pub struct GuardianKeys {
    inner: Arc<Inner>,
}

fn unavailable() -> SysError {
    SysError::Io("guardian-keys state unavailable".into())
}

fn hex(k: &PubKey) -> String {
    k.to_hex()
}

fn keys(hexes: impl IntoIterator<Item = String>) -> Vec<PubKey> {
    hexes
        .into_iter()
        .filter_map(|h| PubKey::from_hex(&h).ok())
        .collect()
}

fn secs(t: u64) -> i64 {
    i64::try_from(t).unwrap_or(i64::MAX)
}

/// The ACK a statement's outcome maps to.
fn ack(outcome: &cg::Outcome) -> AckResult {
    match (outcome.result, outcome.reason) {
        (Verdict::Accepted, _) => AckResult::Applied,
        (Verdict::Dropped, Reason::Duplicate) => AckResult::Unchanged,
        (Verdict::Dropped, Reason::NotNewer | Reason::TieNotNewer) => AckResult::Superseded,
        (Verdict::Dropped, _) => AckResult::Refused,
    }
}

/// The document a ward paired before the section starts from.
fn migrated_doc(pairing: &Pairing) -> Doc {
    Doc {
        v: DOC_VERSION,
        state: migrated(pairing),
        pin_attempts: PinAttempts::default(),
        paired_guardian: Some(hex(&pairing.guardian_pubkey)),
    }
}

/// The state a ward paired before the section starts from: the pinned
/// guardian is its one root, as of the pairing; its relays are the pairing's.
fn migrated(pairing: &Pairing) -> GuardianState {
    let paired_at = secs(pairing.paired_at);
    let mut state = cg::pair_from_seal(
        &hex(&pairing.guardian_pubkey),
        None,
        pairing.relays.clone(),
        paired_at,
    )
    .expect("an offer with no approval always pins its seal author")
    .state;
    state.relays_updated_at = secs(pairing.relays_updated_at.unwrap_or(0));
    state
}

impl GuardianKeys {
    fn new(
        store: Arc<dyn GuardianStateStore>,
        paired_at: i64,
        paired_guardian: String,
        held: Held,
    ) -> Self {
        GuardianKeys {
            inner: Arc::new(Inner {
                store,
                paired_at,
                paired_guardian,
                held: Mutex::new(held),
                staged: Mutex::new(None),
                token_file: Mutex::new(None),
            }),
        }
    }

    /// The adapter for a paired ward, from its pairing record.
    ///
    /// - No state document: the ward paired before the section (or by a path
    ///   that does not write one, such as Android's `bunker://` link). Its
    ///   state is initialised with the pairing's guardian as its one root,
    ///   the pairing's `paired_at` and relays, and written. Clause floors are
    ///   not copied: the clause store seeds each slot the first time a clause
    ///   for it arrives ([`SlotSeed`]).
    /// - A document from an earlier pairing (its `pairedAt` is below the
    ///   record's): replaced the same way. The record's `paired_at` and the
    ///   state's must therefore agree: after [`Self::from_paired`], the
    ///   pairing record MUST carry `PairDecision::paired_at()`.
    /// - A document that is neither (the same `pairedAt` with another
    ///   pinned guardian, or a `pairedAt` above the record's): unavailable,
    ///   fail closed, left on disk. Re-migrating it would re-pin the
    ///   record's guardian over a confirmed root move or removal (review
    ///   batch 2, core L4). Every re-pair path clears the state first.
    /// - Once loaded, every broker built for this ward must carry the port
    ///   (module docs, *Invariant*).
    /// - A document that cannot be read or parsed: unavailable, fail closed
    ///   (module docs). It is left as it is on disk.
    ///
    /// The relay list follows the pairing record, which the transport reads
    /// and a pasted relay code changes: a list that differs from the state's
    /// is recorded as a change, so rule 11's previous-relays window covers it.
    pub fn load(store: Arc<dyn GuardianStateStore>, pairing: &Pairing, now: u64) -> Self {
        Self::load_with(store, pairing, now, true)
    }

    /// [`Self::load`] for a process that must never write the state: another
    /// process (the warden) is its one writer, so a second writer would lose
    /// that process's changes. Nothing is migrated or recorded on disk, and
    /// every call that would change the state fails. For a privileged helper
    /// that only reads it (Linux `charter-presence` listing what is pending,
    /// `charter-pair` checking a pasted relay code against the signer set).
    pub fn peek(store: Arc<dyn GuardianStateStore>, pairing: &Pairing, now: u64) -> Self {
        Self::load_with(Arc::new(ReadOnly(store)), pairing, now, false)
    }

    fn load_with(
        store: Arc<dyn GuardianStateStore>,
        pairing: &Pairing,
        now: u64,
        persist: bool,
    ) -> Self {
        let paired_at = secs(pairing.paired_at);
        let guardian = hex(&pairing.guardian_pubkey);
        let held = match store.load() {
            Err(e) => {
                eprintln!("charter: guardian-keys state unreadable ({e}): failing closed");
                Held::Unavailable("unreadable")
            }
            Ok(None) => Held::Ready(Self::migrate(&*store, pairing, persist)),
            Ok(Some(text)) => match serde_json::from_str::<Doc>(&text) {
                Err(e) => {
                    eprintln!("charter: guardian-keys state corrupt ({e}): failing closed");
                    Held::Unavailable("corrupt")
                }
                Ok(doc) if doc.v != DOC_VERSION => {
                    eprintln!(
                        "charter: guardian-keys state version {} unknown: failing closed",
                        doc.v
                    );
                    Held::Unavailable("unknown version")
                }
                Ok(mut doc) => match doc_pairing(&doc, paired_at, &guardian) {
                    DocPairing::Same => {
                        if follow_pairing_relays(&mut doc.state, pairing, now) && persist {
                            if let Err(e) = store.save(&to_json(&doc)) {
                                eprintln!(
                                    "charter: could not record a relay change in the \
                                     guardian-keys state ({e}); retrying on the next write"
                                );
                            }
                        }
                        Held::Ready(doc)
                    }
                    DocPairing::Earlier => {
                        eprintln!(
                            "charter: guardian-keys state belongs to an earlier pairing; \
                             starting afresh from the pairing record"
                        );
                        Held::Ready(Self::migrate(&*store, pairing, persist))
                    }
                    DocPairing::Other => {
                        eprintln!(
                            "charter: guardian-keys state belongs to another pairing \
                             than the record's: failing closed"
                        );
                        Held::Unavailable("another pairing")
                    }
                },
            },
        };
        Self::new(store, paired_at, guardian, held)
    }

    /// The migrated document for `pairing`, written when `persist`.
    fn migrate(store: &dyn GuardianStateStore, pairing: &Pairing, persist: bool) -> Doc {
        let doc = migrated_doc(pairing);
        if persist {
            if let Err(e) = store.save(&to_json(&doc)) {
                // Derived from the pairing record alone, so the next load
                // derives it again.
                eprintln!(
                    "charter: could not write the migrated guardian-keys state \
                     ({e}); retrying on the next write"
                );
            }
        }
        doc
    }

    /// Persist the state a scan-to-pair produced ([`scan_to_pair`]) and build
    /// the adapter. Nothing is pinned unless this succeeds; the pairing
    /// record the platform writes next must carry `paired.paired_at()`.
    pub fn from_paired(
        store: Arc<dyn GuardianStateStore>,
        paired: PairDecision,
    ) -> SysResult<Self> {
        let guardian = hex(&paired.root);
        let doc = Doc {
            v: DOC_VERSION,
            state: paired.state,
            pin_attempts: PinAttempts::default(),
            paired_guardian: Some(guardian.clone()),
        };
        store.save(&to_json(&doc))?;
        let paired_at = doc.state.paired_at;
        Ok(Self::new(store, paired_at, guardian, Held::Ready(doc)))
    }

    /// Make THIS adapter follow a changed pairing record (a pasted relay code
    /// applied to the record while the ward runs): the relay list and
    /// `relays_updated_at`, recording the replaced list for rule 11's
    /// previous-relays window, as [`Self::load`] does at start. Under the
    /// adapter's own lock, persisted before it is swapped in, so a platform
    /// never builds a second adapter for the same ward (two instances would
    /// overwrite each other's writes).
    ///
    /// `Ok(false)`: nothing to follow, or the record belongs to another
    /// pairing (`paired_at` differs; that is a re-pair, which needs a new
    /// adapter after an unpair, not this). `Err`: unavailable, or the write
    /// failed and nothing changed.
    pub fn follow_pairing(&self, pairing: &Pairing, now: u64) -> SysResult<bool> {
        if !self.serves(pairing) {
            return Ok(false);
        }
        self.commit(|d| {
            let mut next = d.clone();
            let mut changed = follow_pairing_relays(&mut next.state, pairing, now);
            // The same list re-applied by a newer code still moves the floor.
            let updated = secs(pairing.relays_updated_at.unwrap_or(0));
            if updated > next.state.relays_updated_at {
                next.state.relays_updated_at = updated;
                changed = true;
            }
            if changed {
                (Some(next), true)
            } else {
                (None, false)
            }
        })
    }

    /// Also show the presence token from `file` (a `0600` file another
    /// process may display). The adapter's own state stays the gate.
    pub fn set_token_file(&self, file: PresenceTokenStore) {
        *self.inner.token_file.lock().expect("token file lock") = Some(file);
    }

    /// Whether `pairing` is the record this adapter was loaded for.
    fn serves(&self, pairing: &Pairing) -> bool {
        secs(pairing.paired_at) == self.inner.paired_at
            && hex(&pairing.guardian_pubkey) == self.inner.paired_guardian
    }

    fn held(&self) -> MutexGuard<'_, Held> {
        self.inner.held.lock().expect("guardian state lock")
    }

    /// Whether the state could be read (see the module docs).
    pub fn is_available(&self) -> bool {
        matches!(*self.held(), Held::Ready(_))
    }

    /// Compute a change on a copy of the document, write it, then swap it in.
    /// `f` returns `None` to change nothing.
    fn commit<R>(&self, f: impl FnOnce(&Doc) -> (Option<Doc>, R)) -> SysResult<R> {
        let mut held = self.held();
        let Held::Ready(doc) = &mut *held else {
            return Err(unavailable());
        };
        let (next, r) = f(doc);
        if let Some(next) = next {
            if next != *doc {
                self.inner.store.save(&to_json(&next))?;
                *doc = next;
            }
        }
        Ok(r)
    }

    fn read<R>(&self, f: impl FnOnce(&Doc) -> R) -> Option<R> {
        match &*self.held() {
            Held::Ready(doc) => Some(f(doc)),
            Held::Unavailable(_) => None,
        }
    }

    /// The whole state as the golden vectors project it: roots, signers with
    /// their roots, pending statements, held clauses with who set them
    /// (rule 5), recipients. `None` when unavailable.
    pub fn project(&self, now: u64) -> Option<cg::ProjectedState> {
        self.read(|d| cg::project_state(&d.state, secs(now)))
    }

    /// The pending co-guardian, removal and move statements, for the ward's
    /// screen when a guardian asks to confirm one in person. Each `d` is what
    /// [`Self::mint_presence_token`] binds a token to.
    pub fn pending_statements(&self, now: u64) -> Vec<cg::state::ProjectedPending> {
        self.project(now).map(|p| p.pending).unwrap_or_default()
    }

    /// Every key in the signer set (pinned roots and their approved working
    /// keys), sorted. Empty when unavailable. For the offline unlock, which
    /// accepts a code from any of them (open question "Offline unlock with
    /// more than one guardian key").
    pub fn signer_keys(&self) -> Vec<PubKey> {
        self.read(|d| keys(cg::signer_set(&d.state)))
            .unwrap_or_default()
    }

    /// The label a working key's approval gave it, if any (rule 5: shown next
    /// to the key's root as a short npub, never alone, since a label can be
    /// spoofed). Departed keys keep theirs, so a rule they set still says who.
    /// Made safe to show with [`clean_label`]; `None` when nothing is left.
    pub fn signer_label(&self, key: &PubKey) -> Option<String> {
        let key = hex(key);
        self.read(|d| {
            d.state
                .keys
                .iter()
                .find(|k| k.key == key)
                .and_then(|k| k.label.as_deref())
                .map(clean_label)
                .filter(|l| !l.is_empty())
        })
        .flatten()
    }

    /// The departed keys owed one last STATUS that shows their departure
    /// (contract, *STATUS*), and what it shows, read together. `None` when
    /// none is owed or the state is unavailable.
    ///
    /// Interim reading (open question "A departed key's last STATUS"): the
    /// departed key gets [`Farewell::to_json`] only, never the STATUS the
    /// signer set's recipients get (usage, apps, the relay list). Send it to
    /// each key on its own and pass the keys a relay took to
    /// [`Self::mark_status_sent`].
    pub fn farewell(&self) -> Option<Farewell> {
        self.read(|d| {
            let live = cg::recipients(&d.state);
            let recipients = keys(
                d.state
                    .farewell
                    .iter()
                    .filter(|k| !live.contains(k))
                    .cloned(),
            );
            (!recipients.is_empty()).then(|| Farewell {
                recipients,
                guardians: status_guardians_of(&d.state),
            })
        })
        .flatten()
    }

    /// The last STATUS reached `sent` (departed keys from
    /// [`Self::farewell`]): only they are no longer owed it. A key that
    /// departed after the farewell was read stays owed (review batch 2,
    /// core L3).
    pub fn mark_status_sent(&self, sent: &[PubKey], now: u64) -> SysResult<()> {
        let sent: Vec<String> = sent.iter().map(hex).collect();
        self.commit(|d| {
            if !d.state.farewell.iter().any(|k| sent.contains(k)) {
                return (None, ());
            }
            let step = cg::mark_status_sent_to(&d.state, &sent, secs(now));
            (Some(with_state(d, step.state)), ())
        })
    }

    /// Whether a guardian PIN is set (Android: none set, no token can be
    /// minted, and the guardian app sets one first).
    pub fn has_guardian_pin(&self) -> bool {
        self.read(|d| d.state.guardian_pin.is_some())
            .unwrap_or(false)
    }

    /// Check an entered guardian PIN (Android). The backoff is checked and
    /// the attempt COUNTED AND WRITTEN in one locked step before scrypt runs,
    /// so killing the ward mid-check (or a power cut) still costs the
    /// attempt, and entries made at once can't all pass one backoff gate
    /// (review batch 2, core M1). A correct PIN resets the counter.
    pub fn verify_guardian_pin(&self, pin: &str, now: u64) -> PinCheck {
        let _one = PIN_CHECK.lock().unwrap_or_else(PoisonError::into_inner);
        let now = secs(now);
        let gate = self.commit(|d| {
            let Some(record) = d.state.guardian_pin.clone() else {
                return (None, PinGate::NoPin);
            };
            let attempts = d.pin_attempts;
            let next_try = cg::pin_next_try_at(attempts.failures, attempts.last_failure_at);
            if attempts.failures >= cg::pin::PIN_FREE_FAILURES && now < next_try {
                return (None, PinGate::Wait(next_try));
            }
            let counted = PinAttempts {
                failures: attempts.failures.saturating_add(1),
                last_failure_at: now,
            };
            let mut next = d.clone();
            next.pin_attempts = counted;
            (Some(next), PinGate::Check(Box::new(record), counted))
        });
        let (record, counted) = match gate {
            Err(_) => return PinCheck::Unavailable,
            Ok(PinGate::NoPin) => return PinCheck::NoPin,
            Ok(PinGate::Wait(until)) => return PinCheck::Wait { until },
            Ok(PinGate::Check(record, counted)) => (record, counted),
        };
        if cg::verify_pin(&record, pin) {
            if let Err(e) = self.commit(|d| {
                let mut next = d.clone();
                next.pin_attempts = PinAttempts::default();
                (Some(next), ())
            }) {
                // The counter stays raised: only ever more backoff.
                eprintln!("charter: could not reset the guardian PIN counter ({e})");
            }
            return PinCheck::Correct;
        }
        PinCheck::Wrong {
            failures: counted.failures,
            next_try_at: cg::pin_next_try_at(counted.failures, now),
        }
    }

    /// Mint a presence token after the platform's privilege check (an
    /// administrator through polkit on Linux). `bound_to` is the `d` of the
    /// one pending statement it may confirm, or `None` for any (open question
    /// "Presence confirmation details"). A new token replaces any earlier
    /// one, so showing a new code always invalidates the old. The token is
    /// recorded in the persisted state before it is returned.
    pub fn mint_presence_token(
        &self,
        bound_to: Option<&str>,
        now: u64,
        fill: &mut dyn FnMut(&mut [u8]),
    ) -> Result<MintedToken, PresenceError> {
        // Checked before the shown copy is replaced, so a refused request
        // never takes down a code that still works (review batch 2,
        // platforms I3). The commit below checks again.
        let pending = self.read(|d| {
            bound_to.is_none_or(|b| {
                cg::tick(&d.state, secs(now))
                    .state
                    .pending
                    .iter()
                    .any(|p| p.d == b)
            })
        });
        match pending {
            None => return Err(PresenceError::Unavailable),
            Some(false) => return Err(PresenceError::NotPending),
            Some(true) => {}
        }
        let file = self
            .inner
            .token_file
            .lock()
            .expect("token file lock")
            .clone();
        let token = match &file {
            Some(f) => {
                f.mint(now, bound_to, fill)
                    .map_err(|_| PresenceError::Failed)?
                    .token
            }
            None => {
                let mut random = [0u8; charter_sys::presence_token::PRESENCE_TOKEN_BYTES];
                fill(&mut random);
                random.iter().map(|b| format!("{b:02x}")).collect()
            }
        };
        let registered = self.commit(|d| {
            let mut base = d.state.clone();
            base.tokens.clear();
            let step = cg::register_presence_token(&base, &token, bound_to, secs(now));
            if step.outcome.accepted() {
                (Some(with_state(d, step.state)), Ok(()))
            } else {
                (None, Err(step.outcome.reason))
            }
        });
        let fail = |e: PresenceError| {
            if let Some(f) = &file {
                let _ = f.clear();
            }
            Err(e)
        };
        match registered {
            Ok(Ok(())) => Ok(MintedToken {
                token,
                bound_to: bound_to.map(str::to_owned),
                minted_at: now,
                expires_at: now + cg::TOKEN_LIFETIME_SECONDS as u64,
            }),
            Ok(Err(Reason::NotPending)) => fail(PresenceError::NotPending),
            Ok(Err(_)) => fail(PresenceError::Failed),
            Err(_) => fail(PresenceError::Failed),
        }
    }

    /// [`Self::mint_presence_token`] gated by the guardian PIN (Android): no
    /// PIN set, no token; a wrong PIN counts toward the backoff.
    pub fn mint_presence_token_with_pin(
        &self,
        pin: &str,
        bound_to: Option<&str>,
        now: u64,
        fill: &mut dyn FnMut(&mut [u8]),
    ) -> Result<MintedToken, PresenceError> {
        match self.verify_guardian_pin(pin, now) {
            PinCheck::Correct => self.mint_presence_token(bound_to, now, fill),
            other => Err(PresenceError::Pin(other)),
        }
    }

    /// The statement or confirmation's step, computed on the current state,
    /// written, and swapped in.
    fn step_inbound(&self, inbound: &GuardianSealed, now: u64) -> SysResult<GuardianApplied> {
        let value = serde_json::to_value(inbound.event())
            .map_err(|e| SysError::Io(format!("statement to JSON: {e}")))?;
        let seal = hex(&inbound.seal_author());
        let kind = inbound.kind();
        let applied = self.commit(|d| {
            let step = match kind {
                kinds::CHARTER_GUARDIAN_STATEMENT => {
                    cg::apply_inbound(&d.state, &seal, &value, secs(now))
                }
                kinds::CHARTER_PRESENCE_CONFIRM => {
                    // A token "minted" after now (the clock was set back
                    // since) has no lifetime left to trust (review batch 2,
                    // core Info 1).
                    let mut base = d.state.clone();
                    base.tokens.retain(|t| t.minted_at <= secs(now));
                    cg::apply_presence_confirm(&base, &seal, &value, secs(now))
                }
                _ => return (None, GuardianApplied::ignored()),
            };
            let applied = GuardianApplied {
                result: ack(&step.outcome),
                departures: keys(step.outcome.departed.clone()),
            };
            (Some(with_state(d, step.state)), applied)
        })?;
        if kind == kinds::CHARTER_PRESENCE_CONFIRM && applied.result == AckResult::Applied {
            self.spend_token_file(inbound.event(), now);
        }
        Ok(applied)
    }

    /// A confirmation spent its token: take the shown copy down too, if it is
    /// still that token (a fresh one minted since is left alone).
    fn spend_token_file(&self, confirm: &NostrEvent, now: u64) {
        let Some(file) = self
            .inner
            .token_file
            .lock()
            .expect("token file lock")
            .clone()
        else {
            return;
        };
        let token = serde_json::from_str::<serde_json::Value>(&confirm.content)
            .ok()
            .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(str::to_owned));
        if let Some(token) = token {
            let _ = file.consume_if_matches(&token, now);
        }
    }
}

/// The store behind [`GuardianKeys::peek`]: reads through, refuses writes.
struct ReadOnly(Arc<dyn GuardianStateStore>);

impl GuardianStateStore for ReadOnly {
    fn load(&self) -> SysResult<Option<String>> {
        self.0.load()
    }
    fn save(&self, _json: &str) -> SysResult<()> {
        Err(SysError::Io("guardian-keys state opened read-only".into()))
    }
    fn clear(&self) -> SysResult<()> {
        Err(SysError::Io("guardian-keys state opened read-only".into()))
    }
}

fn with_state(d: &Doc, state: GuardianState) -> Doc {
    Doc {
        v: d.v,
        state,
        pin_attempts: d.pin_attempts,
        paired_guardian: d.paired_guardian.clone(),
    }
}

fn to_json(doc: &Doc) -> String {
    serde_json::to_string(doc).expect("the guardian-keys document serialises")
}

/// When `key` left the signer set, as far as the state records it: a
/// departed root's removal or move, the tombstone of a withdrawn or removed
/// working key, or the departure of the root a working key was bound to.
fn departed_at(state: &GuardianState, key: &str) -> Option<i64> {
    let root_left = |r: &str| {
        state
            .departed_roots
            .iter()
            .find(|d| d.root == r)
            .map(|d| d.issued_at)
    };
    root_left(key)
        .or_else(|| {
            state
                .tombstones
                .iter()
                .filter(|t| t.key == key)
                .map(|t| t.issued_at)
                .min()
        })
        .or_else(|| {
            state
                .keys
                .iter()
                .find(|k| k.key == key)
                .and_then(|k| root_left(&k.root))
        })
}

/// A guardian-chosen label made safe to show (review batch 2, platforms L7):
/// no control characters (newlines included), no invisible format
/// characters (bidi overrides and isolates, zero-width marks), no line or
/// paragraph separators; trimmed, and at most 64 bytes. A label is always
/// shown beside its root's npub (rule 5); this keeps it from faking one, or
/// a line of its own.
pub fn clean_label(label: &str) -> String {
    let kept: String = label
        .chars()
        .filter(|&c| !c.is_control() && !is_format_or_separator(c))
        .collect();
    let mut out = String::new();
    for c in kept.trim().chars() {
        if out.len() + c.len_utf8() > 64 {
            break;
        }
        out.push(c);
    }
    out.trim_end().to_owned()
}

/// Unicode format characters (category Cf) and the line and paragraph
/// separators (Zl, Zp).
fn is_format_or_separator(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// The STATUS `guardians` for `state`.
fn status_guardians_of(state: &GuardianState) -> StatusGuardians {
    let g = cg::status_guardians(state);
    StatusGuardians {
        roots: g
            .roots
            .into_iter()
            .filter_map(|r| {
                Some(StatusGuardianRoot {
                    root: PubKey::from_hex(&r.root).ok()?,
                    keys: keys(r.keys),
                })
            })
            .collect(),
        pending: g
            .pending
            .into_iter()
            .map(|p| StatusPendingStatement {
                d: p.d,
                kind: match p.kind {
                    cg::PendingType::Coguardian => PendingKind::Coguardian,
                    cg::PendingType::Removal => PendingKind::Removal,
                    cg::PendingType::Move => PendingKind::Move,
                },
                takes_effect_at: p.takes_effect_at.map(|t| t.max(0) as u64),
                expires_at: p.expires_at.max(0) as u64,
            })
            .collect(),
        tombstones: u32::try_from(g.tombstones).unwrap_or(u32::MAX),
        relays: g.relays,
    }
}

/// Make the state's relay list the pairing record's, recording the replaced
/// one for rule 11 as `apply_relays` does. Returns whether anything changed.
fn follow_pairing_relays(state: &mut GuardianState, pairing: &Pairing, now: u64) -> bool {
    if state.relays == pairing.relays {
        return false;
    }
    let old = std::mem::replace(&mut state.relays, pairing.relays.clone());
    state.previous_relays.push(cg::state::PreviousRelays {
        relays: old,
        until: secs(now).saturating_add(cg::PREVIOUS_RELAYS_SECONDS),
    });
    if state.previous_relays.len() > cg::MAX_PREVIOUS_RELAYS {
        let excess = state.previous_relays.len() - cg::MAX_PREVIOUS_RELAYS;
        state.previous_relays.drain(..excess);
    }
    state.relays_updated_at = state
        .relays_updated_at
        .max(secs(pairing.relays_updated_at.unwrap_or(0)));
    true
}

/// A guardian-PIN check's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinCheck {
    Correct,
    /// Wrong; `failures` consecutive so far, the next try allowed at
    /// `next_try_at` (ward time; now, below 5 failures).
    Wrong {
        failures: u64,
        next_try_at: i64,
    },
    /// Backing off: no try is checked before `until`.
    Wait {
        until: i64,
    },
    /// No PIN is set.
    NoPin,
    /// The state could not be read or the attempt could not be counted:
    /// nothing was checked.
    Unavailable,
}

/// What the counted step of a PIN check decided.
enum PinGate {
    NoPin,
    Wait(i64),
    Check(Box<cg::GuardianPinRecord>, PinAttempts),
}

/// The departed keys owed one last STATUS, and the `guardians` it shows
/// ([`GuardianKeys::farewell`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Farewell {
    /// Each is sent [`Self::to_json`] on its own wrap.
    pub recipients: Vec<PubKey>,
    /// Who the ward trusts now: the departure shows as the key's absence.
    pub guardians: StatusGuardians,
}

impl Farewell {
    /// The farewell STATUS body (kind 31114 content): `{ v, machine, ts,
    /// seq?, guardians: { roots, pending, tombstones } }`. No subject, usage,
    /// apps or lock state, and no relay list: a departed key is told only
    /// that it departed (open question "A departed key's last STATUS").
    pub fn to_json(&self, machine: &PubKey, ts: u64, seq: Option<u64>) -> String {
        let mut body = serde_json::json!({
            "v": 1,
            "machine": machine.to_hex(),
            "ts": ts,
            "guardians": {
                "roots": self.guardians.roots,
                "pending": self.guardians.pending,
                "tombstones": self.guardians.tombstones,
            },
        });
        if let Some(seq) = seq {
            body["seq"] = serde_json::json!(seq);
        }
        body.to_string()
    }
}

/// A minted presence token, for the ward to show as a code to scan (and a
/// short form to type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedToken {
    /// 32 lowercase hex characters.
    pub token: String,
    pub bound_to: Option<String>,
    pub minted_at: u64,
    pub expires_at: u64,
}

/// Why no presence token was minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceError {
    /// The guardian PIN said no (Android).
    Pin(PinCheck),
    /// `bound_to` names no pending statement.
    NotPending,
    /// The state could not be read.
    Unavailable,
    /// The token could not be recorded.
    Failed,
}

// ---------------------------------------------------------------------------
// The live signer set
// ---------------------------------------------------------------------------

/// The live [`SignerSet`] over the adapter's state: every query answers for
/// the state as it stands, so a handle wired once (transport, local link)
/// follows every approval and departure.
struct LiveSigners(GuardianKeys);

impl SignerSet for LiveSigners {
    fn is_signer(&self, key: &PubKey) -> bool {
        self.0
            .read(|d| cg::is_signer(&d.state, &hex(key)))
            .unwrap_or(false)
    }
    fn root_of(&self, key: &PubKey) -> Option<PubKey> {
        self.0
            .read(|d| cg::root_of(&d.state, &hex(key)))
            .flatten()
            .and_then(|r| PubKey::from_hex(&r).ok())
    }
    fn is_departed(&self, key: &PubKey) -> bool {
        self.0
            .read(|d| cg::is_departed(&d.state, &hex(key)))
            .unwrap_or(false)
    }
    fn recipients(&self) -> Vec<PubKey> {
        self.0
            .read(|d| keys(cg::recipients(&d.state)))
            .unwrap_or_default()
    }
    fn roots(&self) -> Vec<PubKey> {
        self.0
            .read(|d| keys(d.state.roots.iter().map(|r| r.root.clone())))
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// The port
// ---------------------------------------------------------------------------

impl GuardianStatements for GuardianKeys {
    fn signers(&self) -> Arc<dyn SignerSet> {
        Arc::new(LiveSigners(self.clone()))
    }

    fn accepts_statements(&self) -> bool {
        true
    }

    /// Computes the step, writes the resulting state and swaps it in, in one
    /// locked step, so nothing (the live signer set included) ever sees a
    /// statement's effect before it is on disk. [`Self::apply`] reports the
    /// outcome.
    fn persist(&self, inbound: &GuardianSealed, now: u64) -> SysResult<()> {
        let applied = self.step_inbound(inbound, now)?;
        *self.inner.staged.lock().expect("staged lock") = Some((inbound.event().id, applied));
        Ok(())
    }

    fn apply(&self, inbound: GuardianSealed, now: u64) -> GuardianApplied {
        let staged = self.inner.staged.lock().expect("staged lock").take();
        match staged {
            Some((id, applied)) if id == inbound.event().id => applied,
            // Not persisted through `persist`: do both now.
            _ => self
                .step_inbound(&inbound, now)
                .unwrap_or_else(|_| GuardianApplied::failed()),
        }
    }

    fn advance(&self, now: u64) -> Vec<PubKey> {
        let mut held = self.held();
        if let Held::Unavailable("unreadable") = *held {
            // A transient read error heals here.
            if let Ok(Some(text)) = self.inner.store.load() {
                if let Ok(doc) = serde_json::from_str::<Doc>(&text) {
                    if doc.v == DOC_VERSION
                        && doc_pairing(&doc, self.inner.paired_at, &self.inner.paired_guardian)
                            == DocPairing::Same
                    {
                        eprintln!("charter: guardian-keys state readable again");
                        *held = Held::Ready(doc);
                    }
                }
            }
        }
        let Held::Ready(doc) = &mut *held else {
            return Vec::new();
        };
        let step = cg::tick(&doc.state, secs(now));
        if step.state == doc.state {
            return Vec::new();
        }
        let next = with_state(doc, step.state);
        if let Err(e) = self.inner.store.save(&to_json(&next)) {
            // Applied anyway: the advance is re-derived from the persisted
            // state and the clock after a restart.
            eprintln!("charter: could not write a guardian-keys timer advance ({e})");
        }
        *doc = next;
        keys(step.outcome.departed)
    }

    fn judge_clause(
        &self,
        clause: &ClauseCandidate<'_>,
        seed: SlotSeed,
        now: u64,
    ) -> Option<ClauseGate> {
        let author = hex(&clause.author);
        let seal = hex(&clause.seal_author);
        let gate = self.commit(|d| {
            let mut base = d.state.clone();
            if cg::held_clause(&base, clause.subject_hex, clause.kind).is_none() {
                match seed {
                    SlotSeed::Empty => {}
                    SlotSeed::Unreadable => return (None, ClauseGate::Unknown),
                    SlotSeed::Held { signer, bound } => {
                        // A slot this state has never judged (the ward paired
                        // before the section): the store's floor, and its
                        // signer, carry over. A signer no longer in the set
                        // departed before the slot was seeded, so no departure
                        // clamped it: its floor is min(its issuedAt, the
                        // departure), as rule 4 has it, or now when the
                        // departure time is not known. A far-future clause
                        // from a departed key can't pin the slot (review
                        // batch 2, core L1).
                        let signer = hex(&signer);
                        let root = cg::root_of(&base, &signer).unwrap_or_else(|| signer.clone());
                        let issued_at = secs(bound);
                        let floor = (!cg::is_signer(&base, &signer)).then(|| {
                            issued_at.min(departed_at(&base, &signer).unwrap_or(secs(now)))
                        });
                        base.clauses.push(cg::state::HeldClause {
                            subject: clause.subject_hex.to_owned(),
                            kind: clause.kind.to_owned(),
                            signer,
                            root,
                            issued_at,
                            floor,
                        });
                    }
                }
            }
            let input = cg::ClauseInput {
                author: &author,
                seal_author: &seal,
                subject: clause.subject_hex,
                kind: clause.kind,
                issued_at: secs(clause.issued_at),
            };
            let step = cg::accept_clause(&base, &input, secs(now));
            if step.outcome.accepted() {
                return (Some(with_state(d, step.state)), ClauseGate::Accepted);
            }
            let gate = match step.outcome.reason {
                Reason::NotNewer => {
                    let held_is_this = cg::held_clause(&base, clause.subject_hex, clause.kind)
                        .is_some_and(|h| {
                            h.signer == author && h.issued_at == secs(clause.issued_at)
                        });
                    ClauseGate::NotNewer { held_is_this }
                }
                _ => ClauseGate::Refused,
            };
            (None, gate)
        });
        Some(gate.unwrap_or(ClauseGate::Failed))
    }

    fn record_relays(&self, change: &RelaysChange<'_>, now: u64) -> RelaysRecord {
        // The record the change would be saved to is another pairing's (a
        // re-pair replaced it while this warden runs): refuse (review batch
        // 2, platforms L5).
        if secs(change.paired_at) != self.inner.paired_at {
            return RelaysRecord::Refused;
        }
        let author = hex(&change.author);
        let seal = hex(&change.seal_author);
        let pin = change.pin.and_then(|p| serde_json::to_value(p).ok());
        let input = cg::RelaysInput {
            author: &author,
            channel: cg::RelaysChannel::Wrapped { seal_author: &seal },
            ts: secs(change.ts),
            relays: change.relays,
            guardian_pin: pin.as_ref(),
        };
        let recorded = self.commit(|d| {
            let step = cg::apply_relays(&d.state, &input, secs(now));
            match (step.outcome.result, step.outcome.reason) {
                (Verdict::Accepted, _) => (Some(with_state(d, step.state)), RelaysRecord::Recorded),
                (_, Reason::NotNewer) => (None, RelaysRecord::AlreadyHeld),
                _ => (None, RelaysRecord::Refused),
            }
        });
        recorded.unwrap_or(RelaysRecord::Failed)
    }

    fn status_relays(&self, now: u64) -> Vec<Vec<String>> {
        let now = secs(now);
        self.read(|d| {
            if d.state.roots.len() < 2 {
                return Vec::new();
            }
            d.state
                .previous_relays
                .iter()
                .rev()
                .filter(|p| now < p.until)
                .map(|p| p.relays.clone())
                .collect()
        })
        .unwrap_or_default()
    }

    fn status_guardians(&self, _now: u64) -> Option<StatusGuardians> {
        self.read(|d| status_guardians_of(&d.state))
    }

    fn clear(&self) -> SysResult<()> {
        let mut held = self.held();
        let r = self.inner.store.clear();
        *held = Held::Unavailable("released");
        if let Some(f) = self
            .inner
            .token_file
            .lock()
            .expect("token file lock")
            .as_ref()
        {
            let _ = f.clear();
        }
        r
    }
}

// ---------------------------------------------------------------------------
// Scan-to-pair and RELEASE: the decisions both platforms make
// ---------------------------------------------------------------------------

/// What a scan-to-pair pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairDecision {
    /// The root to pin: the seal author for an offer with no approval, the
    /// approving root R for one sealed by a working key (never that W).
    pub root: PubKey,
    /// The working key bound at pairing, when the offer carried an approval.
    pub working_key: Option<PubKey>,
    /// The new pairing's state, for [`GuardianKeys::from_paired`].
    pub state: GuardianState,
}

impl PairDecision {
    /// The pairing time the pairing record must carry.
    pub fn paired_at(&self) -> u64 {
        self.state.paired_at.max(0) as u64
    }
}

/// Decide a PAIR_OFFER (contract, *Scan-to-pair with a working key*): which
/// root an unpaired ward pins. `token` is the one the ward minted and showed.
///
/// - A signed inner offer (the guardian app signs it) goes through
///   `charter-guardians`' `pair_from_offer`: the inner signature, kind,
///   marker, seal == author, token (constant time, 600-second lifetime from
///   the mint and from the offer's `ts`, no future `ts`) and the approval.
/// - An unsigned inner offer (the seal authenticates it) gets the same token
///   checks here and pins its seal author, but only with no `approval`: an
///   approval is refused without a signed offer (fail closed).
///
/// A contested token (two keys, one token) is the caller's to detect, as
/// before.
pub fn scan_to_pair(
    offer: &ReceivedPairOffer,
    token: &cg::PairToken<'_>,
    now: u64,
) -> Result<PairDecision, Reason> {
    let seal = hex(&offer.seal_author);
    let paired = match &offer.inner {
        Some(inner) => {
            let value = serde_json::to_value(inner).map_err(|_| Reason::BadOffer)?;
            cg::pair_from_offer(&seal, &value, token, secs(now))?
        }
        None => {
            if offer.offer.approval.is_some() {
                return Err(Reason::BadOffer);
            }
            let now_s = secs(now);
            let ts = secs(offer.offer.ts);
            if !charter_transport::pair_offer::token_matches(token.token, &offer.offer.token)
                || now_s >= token.minted_at + cg::TOKEN_LIFETIME_SECONDS
                || ts > now_s
                || now_s - ts >= cg::TOKEN_LIFETIME_SECONDS
            {
                return Err(Reason::TokenInvalid);
            }
            cg::pair_from_seal(&seal, None, offer.offer.relays.clone(), now_s)?
        }
    };
    let root = PubKey::from_hex(&paired.root).map_err(|_| Reason::BadOffer)?;
    let working_key = match &paired.working_key {
        Some(w) => Some(PubKey::from_hex(w).map_err(|_| Reason::BadOffer)?),
        None => None,
    };
    Ok(PairDecision {
        root,
        working_key,
        state: paired.state,
    })
}

/// Why a RELEASE was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseRefused {
    /// The seal's author is not the inner event's author.
    SealMismatch,
    /// More than one root is pinned: one parent's remote unpair would remove
    /// the other's control (open question "A remote unpair with more than one
    /// root").
    MoreThanOneRoot,
    /// `verify_release` refused it (kind, id, signature, signer set, machine,
    /// freshness).
    Invalid,
    /// Issued before the current pairing.
    BeforePairing,
}

/// The RELEASE decision (the remote unpair), shared by the broker and the
/// Android warden: sealed by its own author, signed by a key in the signer
/// set, refused while more than one root is pinned, bound to this machine,
/// fresh, and not older than the current pairing. `paired_at` 0 (a legacy
/// pairing record without one) admits every release, as before.
pub fn release_gate(
    event: &NostrEvent,
    seal_author: &PubKey,
    signers: &dyn SignerSet,
    machine: &PubKey,
    paired_at: u64,
    now: u64,
) -> Result<ReleasePayload, ReleaseRefused> {
    if *seal_author != event.pubkey {
        return Err(ReleaseRefused::SealMismatch);
    }
    if signers.roots().len() > 1 {
        return Err(ReleaseRefused::MoreThanOneRoot);
    }
    charter_verify::verify_release(event, signers, machine, now)
        .map_err(|_| ReleaseRefused::Invalid)?;
    let payload = ReleasePayload::from_json(&event.content).map_err(|_| ReleaseRefused::Invalid)?;
    if payload.issued_at < paired_at {
        return Err(ReleaseRefused::BeforePairing);
    }
    Ok(payload)
}
