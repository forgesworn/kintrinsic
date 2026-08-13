//! The frozen D-Bus contract constants. **Single source of truth.** Every
//! consumer — `charterd`, `charter-cli`, the GUI core, the shipped D-Bus
//! policy, and the VM introspection assertion — imports these exact values.
//! Do not redefine divergent names per crate.

/// System-bus well-known name.
pub const BUS_NAME: &str = "org.forgesworn.charterd";
/// Object path.
pub const OBJECT_PATH: &str = "/org/forgesworn/charterd";
/// Interface name.
pub const INTERFACE: &str = "org.forgesworn.Charter1";

// --- Methods ---------------------------------------------------------------

/// `SubmitRequest(op:s, params_json:s) -> req_id:s` — the only privileged-write
/// method; builds + publishes a REQUEST and enacts nothing pre-grant.
pub const METHOD_SUBMIT_REQUEST: &str = "SubmitRequest";
/// `QueryStatus(req_id:s) -> status_json:s` — read-only (`""` = all).
pub const METHOD_QUERY_STATUS: &str = "QueryStatus";
/// `ListRequests(limit:u) -> json:s` — read-only, newest first.
pub const METHOD_LIST_REQUESTS: &str = "ListRequests";
/// `CancelRequest(req_id:s) -> b` — withdraw a pending request you own.
pub const METHOD_CANCEL_REQUEST: &str = "CancelRequest";
/// `TimeLeft() -> time_left_json:s` — read-only.
pub const METHOD_TIME_LEFT: &str = "TimeLeft";

// --- Signals ---------------------------------------------------------------

/// `RequestUpdated(req_id:s, status_json:s)`.
pub const SIGNAL_REQUEST_UPDATED: &str = "RequestUpdated";
/// `TimeLeftChanged(time_left_json:s)`.
pub const SIGNAL_TIME_LEFT_CHANGED: &str = "TimeLeftChanged";
/// `LockStateChanged(locked:b, reason_json:s)`.
pub const SIGNAL_LOCK_STATE_CHANGED: &str = "LockStateChanged";

/// The complete, frozen method allowlist. Introspection MUST equal exactly
/// this set — no more, no less.
pub const METHODS: [&str; 5] = [
    METHOD_SUBMIT_REQUEST,
    METHOD_QUERY_STATUS,
    METHOD_LIST_REQUESTS,
    METHOD_CANCEL_REQUEST,
    METHOD_TIME_LEFT,
];

/// The complete, frozen signal allowlist.
pub const SIGNALS: [&str; 3] = [
    SIGNAL_REQUEST_UPDATED,
    SIGNAL_TIME_LEFT_CHANGED,
    SIGNAL_LOCK_STATE_CHANGED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_is_frozen() {
        assert_eq!(BUS_NAME, "org.forgesworn.charterd");
        assert_eq!(OBJECT_PATH, "/org/forgesworn/charterd");
        assert_eq!(INTERFACE, "org.forgesworn.Charter1");
        assert_eq!(METHODS.len(), 5);
        assert_eq!(SIGNALS.len(), 3);
    }

    #[test]
    fn no_enact_or_approve_method_exists() {
        // No-local-authority invariant: the only privileged-write surface is
        // SubmitRequest (publish-only). There is no "just do it" method.
        for m in METHODS {
            let lower = m.to_lowercase();
            assert!(!lower.contains("enact"), "forbidden method {m}");
            assert!(!lower.contains("approve"), "forbidden method {m}");
            assert!(!lower.contains("install"), "forbidden method {m}");
            assert!(!lower.contains("grant"), "forbidden method {m}");
        }
    }
}
