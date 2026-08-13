//! The D-Bus surface contract. The live zbus server is wired behind the `real`
//! feature on a real bus; here we pin the **method/signal allowlist** to the
//! single source of truth in `charter-ipc` and the caller authorization, both
//! testable without a bus. Introspection MUST equal exactly this set.

use charter_ipc::contract;

/// The exact set of methods the daemon exposes (no more, no less).
pub fn exposed_methods() -> &'static [&'static str] {
    &contract::METHODS
}

/// The exact set of signals the daemon emits.
pub fn exposed_signals() -> &'static [&'static str] {
    &contract::SIGNALS
}

/// Whether a method name is part of the contract surface.
pub fn is_known_method(name: &str) -> bool {
    contract::METHODS.contains(&name)
}

// DELETED: `caller_is_authorized(caller_uid, managed_uid)` (S8, review
// 2026-08-07).
//
// It implemented exactly the ownership check the D-Bus surface needed, it had
// a unit test pinning its behaviour, and the live server never called it once.
// That is worse than having no check at all: a reviewer reading this module,
// or a coverage report counting its test, would conclude the surface was
// scoped when `ListRequests` was handing every local uid every other uid's
// pending requests.
//
// The real check now lives where it can actually be reached — on the record
// itself (`RequestRecord::visible_to`, charter-spine), called by
// `query_status` / `list_requests` / `cancel_request` with a kernel-attested
// uid. It also fixes what this signature got wrong: there is no single
// "managed uid" on a family laptop (there are several children), and root has
// to be exempt because charterd runs as root.
//
// If a caller-authorization helper is ever wanted here again, wire it in the
// same commit that adds it.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn introspection_equals_charter_ipc_allowlist() {
        // The exposed surface is byte-for-byte the charter-ipc contract.
        assert_eq!(exposed_methods(), &contract::METHODS);
        assert_eq!(exposed_signals(), &contract::SIGNALS);
        assert_eq!(exposed_methods().len(), 5);
    }

    #[test]
    fn no_direct_enact_method() {
        for m in exposed_methods() {
            let l = m.to_lowercase();
            assert!(!l.contains("enact") && !l.contains("approve") && !l.contains("grant"));
        }
        assert!(is_known_method("SubmitRequest"));
        assert!(!is_known_method("Enact"));
    }

    /*
     * The ownership rule that REPLACED this module's dead `caller_is_authorized`
     * (S8). Tested against the type the live server actually consults, so this
     * test cannot pass while the surface is unscoped — which is precisely how
     * the old one managed to.
     */
    #[test]
    fn a_request_is_visible_to_its_owner_and_to_root_and_to_nobody_else() {
        use charter_primitives::{Nonce, ReqId};
        use charter_proto::OpType;
        use charter_spine::lifecycle::{RequestRecord, RequestState};

        let rec = |caller_uid| RequestRecord {
            req_id: ReqId::from_bytes([1; 32]),
            nonce: Nonce::from_bytes([2; 32]),
            op: OpType::TimeExtend,
            state: RequestState::Pending,
            created_at: 0,
            detail: None,
            source_path: None,
            caller_uid,
        };

        assert!(rec(Some(1000)).visible_to(1000), "your own ask");
        assert!(!rec(Some(1000)).visible_to(1001), "not your sibling's");
        assert!(rec(Some(1000)).visible_to(0), "root sees everything");
        // Unowned (a single-user platform, or a record from before the field
        // existed) belongs to nobody: root only, never handed to a local
        // account on the strength of a missing field.
        assert!(!rec(None).visible_to(1000));
        assert!(rec(None).visible_to(0));
    }
}
