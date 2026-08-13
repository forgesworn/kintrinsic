//! End-to-end behaviour of the mock `SystemLayer`: durable persistence across a
//! simulated daemon restart (a new `MockSystem` over the same disk handle).

#![cfg(feature = "mock")]

use charter_primitives::ReqId;
use charter_sys::persistence::{ClauseStore, ConsumedIdStore};
use charter_sys::{MockSystem, SystemLayer};

#[test]
fn consumed_ids_durable_across_system_reopen() {
    let sys = MockSystem::new(1000);
    let disk = sys.disk();
    let id = ReqId::from_bytes([9; 32]);
    assert!(sys.consumed_ids().check_and_consume(&id, 99_999).unwrap());
    drop(sys); // restart

    let sys2 = MockSystem::over_disk(2000, disk);
    assert!(
        !sys2.consumed_ids().check_and_consume(&id, 99_999).unwrap(),
        "a consumed id must still be rejected after restart"
    );
}

#[test]
fn clause_rollback_protection_survives_reopen() {
    let sys = MockSystem::new(1000);
    let disk = sys.disk();
    assert!(sys.clauses().put_clause(31113, 500, "{\"x\":1}").unwrap());
    drop(sys);

    let sys2 = MockSystem::over_disk(2000, disk);
    // A lower issuedAt clause is still rejected after restart.
    assert!(!sys2.clauses().put_clause(31113, 400, "{\"x\":2}").unwrap());
    assert_eq!(sys2.clauses().highest_issued_at(31113).unwrap(), Some(500));
}
