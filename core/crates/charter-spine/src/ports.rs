//! Broker output seams: event notifications (D-Bus signals) and the time-left
//! provider (filled by the Phase-6 enforcer).

use std::sync::Mutex;

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
