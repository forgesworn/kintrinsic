//! Signed-clause authentication + rollback protection. A hostile relay can
//! delay/drop a clause but never forge one or roll it back.

#![cfg(feature = "mock")]

use charter_proto::ClauseKind;
use charter_verify::test_support::{ClauseBuilder, TestGuardian};
use charter_verify::{verify_clause, ClauseError};

const NOW: u64 = 1_700_001_000;

#[test]
fn valid_schedule_clause_authenticates() {
    let g = TestGuardian::new();
    let ev = ClauseBuilder::schedule(500).build(&g);
    let pk = g.pubkey();
    let vc = verify_clause(&ev, &pk, ClauseKind::Schedule, None, NOW).unwrap();
    assert_eq!(vc.kind(), ClauseKind::Schedule);
    assert_eq!(vc.issued_at(), 500);
}

#[test]
fn clause_reject_untrusted_signer() {
    // hostile-relay-cannot-set-schedule: a forged "always-allow" clause signed
    // by an attacker key is rejected.
    let pinned = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let ev = ClauseBuilder::schedule(500).build(&attacker);
    let pk = pinned.pubkey();
    assert_eq!(
        verify_clause(&ev, &pk, ClauseKind::Schedule, None, NOW),
        Err(ClauseError::UntrustedSigner)
    );
}

#[test]
fn clause_reject_lower_issued_at() {
    // Rollback: an older legitimately-signed clause is refused once a newer one
    // has been seen.
    let g = TestGuardian::new();
    let ev = ClauseBuilder::schedule(400).build(&g);
    let pk = g.pubkey();
    assert_eq!(
        verify_clause(&ev, &pk, ClauseKind::Schedule, Some(500), NOW),
        Err(ClauseError::StaleIssuedAt)
    );
    // Equal issuedAt is also a rollback.
    let ev_eq = ClauseBuilder::schedule(500).build(&g);
    assert_eq!(
        verify_clause(&ev_eq, &pk, ClauseKind::Schedule, Some(500), NOW),
        Err(ClauseError::StaleIssuedAt)
    );
}

#[test]
fn clause_reject_bad_signature() {
    let g = TestGuardian::new();
    let mut ev = ClauseBuilder::schedule(500).build(&g);
    let mut sig = *ev.sig.as_bytes();
    sig[5] ^= 0x10;
    ev.sig = charter_primitives::Sig::from_bytes(sig);
    let pk = g.pubkey();
    assert_eq!(
        verify_clause(&ev, &pk, ClauseKind::Schedule, None, NOW),
        Err(ClauseError::BadSignature)
    );
}

#[test]
fn clause_reject_kind_mismatch() {
    // A budget clause presented where a schedule is expected is rejected.
    let g = TestGuardian::new();
    let ev = ClauseBuilder::budget(500).build(&g);
    let pk = g.pubkey();
    assert_eq!(
        verify_clause(&ev, &pk, ClauseKind::Schedule, None, NOW),
        Err(ClauseError::BadShape)
    );
}

#[test]
fn newer_clause_after_seen_accepts() {
    let g = TestGuardian::new();
    let ev = ClauseBuilder::schedule(600).build(&g);
    let pk = g.pubkey();
    let vc = verify_clause(&ev, &pk, ClauseKind::Schedule, Some(500), NOW).unwrap();
    assert_eq!(vc.issued_at(), 600);
}
