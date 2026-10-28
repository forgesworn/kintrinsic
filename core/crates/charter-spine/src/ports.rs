//! Broker output seams: event notifications (D-Bus signals) and the time-left
//! provider (filled by the Phase-6 enforcer).

use std::sync::{Arc, Mutex};

use charter_link::AckResult;
use charter_primitives::{PubKey, SignerSet, SinglePinned};
use charter_proto::StatusGuardians;
use charter_sys::SysResult;
use charter_transport::relay_config::PreviousRelays;
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
    /// Signers that left the set as a result, each with the ward time it
    /// left (rule 4). The broker records their clauses' departure floors in
    /// the clause store before it releases the handler turn.
    pub departures: Vec<(PubKey, u64)>,
}

impl GuardianApplied {
    /// Dropped, as an unknown inner kind is.
    pub fn ignored() -> Self {
        GuardianApplied {
            result: AckResult::Ignored,
            departures: Vec::new(),
        }
    }
}

/// The seam between the broker and the guardian-keys state machine.
///
/// The broker owns delivery: it polls kind 31120 and 31121 wraps (relays and
/// the local link), opens them with the seal-exempt unwrap, and hands each to
/// [`GuardianStatements::persist`] then [`GuardianStatements::apply`] under
/// its handler turn, so a statement never interleaves with a clause or grant
/// it could change the verdict on. It also asks [`GuardianStatements::signers`]
/// for the signer set every author check uses.
///
/// [`SinglePinnedGuardians`] is the implementation for a ward that pins one
/// guardian and does not implement the section: the pinned key is the whole
/// signer set, and statements are dropped exactly as an unknown kind is.
pub trait GuardianStatements: Send + Sync {
    /// The current signer set (a snapshot; asked once per inbound event).
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
    /// ending, pending statements expiring), returning the departures they
    /// caused. Called at the start of every broker poll, under the turn.
    fn advance(&self, _now: u64) -> Vec<(PubKey, u64)> {
        Vec::new()
    }

    /// A RELAYS change from a signer applied (rule 11); `previous` is the
    /// list it replaced and how long STATUS should still go out on it.
    fn relays_changed(&self, _previous: PreviousRelays, _now: u64) {}

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
