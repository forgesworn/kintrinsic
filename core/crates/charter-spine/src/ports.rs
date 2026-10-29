//! Broker output seams: event notifications (D-Bus signals) and the time-left
//! provider (filled by the Phase-6 enforcer).

use std::sync::{Arc, Mutex};

use charter_link::AckResult;
use charter_primitives::{PubKey, SignerSet, SinglePinned};
use charter_proto::StatusGuardians;
use charter_sys::SysResult;
use charter_transport::relay_config::GuardianPin;
use charter_transport::GuardianSealed;

use crate::lifecycle::RequestState;

/// Receives lifecycle/lock notifications (the D-Bus signal bridge).
pub trait EventSink: Send + Sync {
    fn request_updated(&self, req_id: &str, state: RequestState);
    fn lock_state_changed(&self, locked: bool, reason: Option<String>);
}

/// A no-op sink.
#[derive(Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn request_updated(&self, _req_id: &str, _state: RequestState) {}
    fn lock_state_changed(&self, _locked: bool, _reason: Option<String>) {}
}

/// A recording sink for tests.
#[derive(Default)]
pub struct RecordingEventSink {
    pub updates: Mutex<Vec<(String, RequestState)>>,
    pub locks: Mutex<Vec<(bool, Option<String>)>>,
}

impl EventSink for RecordingEventSink {
    fn request_updated(&self, req_id: &str, state: RequestState) {
        self.updates
            .lock()
            .expect("lock")
            .push((req_id.to_string(), state));
    }
    fn lock_state_changed(&self, locked: bool, reason: Option<String>) {
        self.locks.lock().expect("lock").push((locked, reason));
    }
}

/// Time-left state. `Unknown` is explicitly NOT "unlimited" (fail-safe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeLeftState {
    Active,
    Locked,
    Unknown,
}

/// A snapshot of remaining time (filled by the Phase-6 enforcer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeLeftSnapshot {
    pub state: TimeLeftState,
    pub effective_seconds: i64,
}

/// Provides the current time-left snapshot.
pub trait TimeLeftProvider: Send + Sync {
    fn snapshot(&self) -> TimeLeftSnapshot;
}

/// The Phase-3 stub: always `Unknown` (NOT unlimited).
#[derive(Default)]
pub struct UnknownTimeLeft;

impl TimeLeftProvider for UnknownTimeLeft {
    fn snapshot(&self) -> TimeLeftSnapshot {
        TimeLeftSnapshot {
            state: TimeLeftState::Unknown,
            effective_seconds: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Guardian roots and working keys: the port the guardian-keys state machine
// plugs into (contract, *Guardian roots and working keys (v1)*).
// ---------------------------------------------------------------------------

/// What the guardian-keys port made of one inbound statement or presence
/// confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardianApplied {
    /// The local-link ACK result (`Applied`, `Unchanged`, `Refused`, …).
    pub result: AckResult,
    /// Signers that left the set as a result (rule 4). Informational: the
    /// clause floors a departure leaves behind are already in the port's
    /// persisted state. A platform closes local-link sessions with them.
    pub departures: Vec<PubKey>,
}

impl GuardianApplied {
    /// Dropped, as an unknown inner kind is.
    pub fn ignored() -> Self {
        GuardianApplied {
            result: AckResult::Ignored,
            departures: Vec::new(),
        }
    }

    /// Not applied: nothing could be persisted (rule 10).
    pub fn failed() -> Self {
        GuardianApplied {
            result: AckResult::Failed,
            departures: Vec::new(),
        }
    }
}

/// A per-child CLAUSE the broker has authenticated (id, signature, kind, the
/// signer-set gate, the 300-second future refusal), for the port's rules 4
/// and 5 to judge against the clause it holds for the same `(subject, kind)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClauseCandidate<'a> {
    pub author: PubKey,
    pub seal_author: PubKey,
    /// 64 lowercase hex.
    pub subject_hex: &'a str,
    /// The clause kind's wire name (`"budget"`, `"apps"`, …).
    pub kind: &'a str,
    pub issued_at: u64,
}

/// What the clause store holds for the candidate's slot. A port that has
/// never judged a clause for the slot (a ward that paired before the
/// guardian-keys section) takes its bar from this, so upgrading never drops a
/// replay floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSeed {
    /// Nothing was ever stored in the slot.
    Empty,
    /// The held clause's signer (a legacy record is the pinned root's) and
    /// the `issuedAt` a new clause must be strictly above.
    Held { signer: PubKey, bound: u64 },
    /// The slot is on disk but could not be read.
    Unreadable,
}

/// The port's verdict on a [`ClauseCandidate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseGate {
    /// Accepted, and the port's state recording it is persisted: store the
    /// body (it may replace a clause dated above it, rule 4).
    Accepted,
    /// At or below the bar. `held_is_this`: the clause the port holds for
    /// the slot is this one (same signer, same `issuedAt`), so a store that
    /// does not have it yet (its write failed after the port's) takes it.
    NotNewer { held_is_this: bool },
    /// Refused (not a signer, seal mismatch, before pairing, future).
    Refused,
    /// The port holds nothing for the slot and the store's slot is
    /// unreadable: no floor is known, so the broker's unreadable-floor rule
    /// applies.
    Unknown,
    /// The port's state could not be read or written: not applied.
    Failed,
}

/// A gift-wrapped RELAYS change that passed every pairing-record check
/// (`apply_signed_relays`), for the port to record (rule 11) before the
/// pairing record is saved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelaysChange<'a> {
    pub author: PubKey,
    pub seal_author: PubKey,
    /// The event's `created_at`.
    pub ts: u64,
    pub relays: &'a [String],
    /// The guardian PIN record it carried, if any. The port keeps it in its
    /// own `0600` state; the pairing record never holds it (review 2a, L2).
    pub pin: Option<&'a GuardianPin>,
    /// The `paired_at` of the pairing record the change would be saved to.
    /// A port serving another pairing refuses it: a re-pair can replace the
    /// record while the old warden is still running (review batch 2,
    /// platforms L5).
    pub paired_at: u64,
}

/// What the port made of a [`RelaysChange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaysRecord {
    /// Recorded and persisted.
    Recorded,
    /// The port already holds this change or a newer one (a re-delivery
    /// after the pairing record's save failed).
    AlreadyHeld,
    /// The port's own checks refused it: the pairing record is not changed.
    Refused,
    /// Could not be persisted: the pairing record is not changed.
    Failed,
}

/// The seam between the broker and the guardian-keys state machine.
///
/// The broker owns delivery: it polls kind 31120 and 31121 wraps (relays and
/// the local link), opens them with the seal-exempt unwrap, and hands each to
/// [`GuardianStatements::persist`] then [`GuardianStatements::apply`] under
/// its handler turn, so a statement never interleaves with a clause or grant
/// it could change the verdict on. It also asks [`GuardianStatements::signers`]
/// for the signer set every author check uses, and
/// [`GuardianStatements::judge_clause`] for the bar a per-child clause must
/// clear.
///
/// [`SinglePinnedGuardians`] is the implementation for a ward that pins one
/// guardian and does not implement the section: the pinned key is the whole
/// signer set, statements are dropped exactly as an unknown kind is, and the
/// clause store's replay floor alone judges clauses.
/// `crate::guardian_keys::GuardianKeys` is the implementation for a ward that
/// does.
pub trait GuardianStatements: Send + Sync {
    /// The signer set, as a LIVE handle: every query on it must answer for
    /// the set as it is at that moment, for the port's whole lifetime, not a
    /// copy taken when this was called. Three holders keep it: the broker
    /// (asked afresh per inbound event), `CharterTransport::set_signer_set`
    /// (recipients of every outbound wrap) and `WardLinkConfig::signers`
    /// (who may complete a local-link handshake). The last two are wired once,
    /// so a snapshot would never see a working key approved afterwards.
    fn signers(&self) -> Arc<dyn SignerSet>;

    /// Whether this ward processes statements and presence confirmations at
    /// all. `false`: the broker does not poll for them, and drops one that
    /// arrives over the local link as `Ignored`, as for any unknown kind.
    fn accepts_statements(&self) -> bool;

    /// Rule 10, "A ward persists a statement before it applies it": durably
    /// record `inbound` (or what applying it needs). Called before
    /// [`Self::apply`], under the handler turn. `Err` means it is NOT applied
    /// and the ACK says `Failed`.
    fn persist(&self, inbound: &GuardianSealed, now: u64) -> SysResult<()>;

    /// Apply a persisted statement or presence confirmation: seal rules, the
    /// statement rules, presence-token checks. Called only after
    /// [`Self::persist`] succeeded.
    fn apply(&self, inbound: GuardianSealed, now: u64) -> GuardianApplied;

    /// Time-driven transitions due at `now` (a removal's notice period
    /// ending, pending statements expiring), returning the signers they made
    /// depart. Called before every handler that consults the signer set,
    /// under the turn.
    fn advance(&self, _now: u64) -> Vec<PubKey> {
        Vec::new()
    }

    /// Rules 4 and 5 for a per-child clause. `None`: this port does not judge
    /// clauses, and the clause store's replay floor does. The machine-wide
    /// `content` slot is never brought here (open question "Rule 4 for the
    /// machine-wide content clause").
    fn judge_clause(
        &self,
        _clause: &ClauseCandidate<'_>,
        _seed: SlotSeed,
        _now: u64,
    ) -> Option<ClauseGate> {
        None
    }

    /// Record a RELAYS change from a signer (rule 11) and the guardian PIN it
    /// carries. Called before the pairing record is saved. A port that keeps
    /// no relay history records nothing, and a PIN is then dropped.
    fn record_relays(&self, _change: &RelaysChange<'_>, _now: u64) -> RelaysRecord {
        RelaysRecord::Recorded
    }

    /// Extra relay lists STATUS should also be published on at `now` (rule
    /// 11: for 7 days after a RELAYS change, on a ward with more than one
    /// root). Empty for a single-root ward.
    fn status_relays(&self, _now: u64) -> Vec<Vec<String>> {
        Vec::new()
    }

    /// STATUS `guardians` for this ward, or `None` to omit the field.
    fn status_guardians(&self, _now: u64) -> Option<StatusGuardians> {
        None
    }

    /// Forget everything (a RELEASE applied): called after the clause store
    /// is cleared and before the pairing record is.
    fn clear(&self) -> SysResult<()> {
        Ok(())
    }
}

/// The ward that pins one guardian and nothing else: today's behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinglePinnedGuardians(pub PubKey);

impl GuardianStatements for SinglePinnedGuardians {
    fn signers(&self) -> Arc<dyn SignerSet> {
        Arc::new(SinglePinned(self.0))
    }
    fn accepts_statements(&self) -> bool {
        false
    }
    fn persist(&self, _inbound: &GuardianSealed, _now: u64) -> SysResult<()> {
        Ok(())
    }
    fn apply(&self, _inbound: GuardianSealed, _now: u64) -> GuardianApplied {
        GuardianApplied::ignored()
    }
}
