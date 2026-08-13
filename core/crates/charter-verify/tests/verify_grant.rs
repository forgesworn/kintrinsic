//! The six §5 grant-verification rules — one named failure case each.

#![cfg(feature = "mock")]

use charter_primitives::{Nonce, ReqId};
use charter_proto::OpType;
use charter_sys::error::SysError;
use charter_sys::persistence::{ConsumedIdStore, MockConsumedIdStore, MockDisk};
use charter_sys::SysResult;
use charter_verify::test_support::{GrantBuilder, TestGuardian};
use charter_verify::{verify_grant, GrantOutcome, VerifyError, VerifyParams};

const NOW: u64 = 1_700_001_000;

fn store() -> MockConsumedIdStore {
    MockConsumedIdStore::new(MockDisk::new())
}

fn rid() -> ReqId {
    ReqId::from_bytes([0xA1; 32])
}
fn non() -> Nonce {
    Nonce::from_bytes([0xB2; 32])
}

#[test]
fn golden_allow_grant_verifies() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    let vg = verify_grant(&ev, &p, &store()).unwrap();
    assert!(matches!(vg.outcome(), GrantOutcome::Allow(_)));
}

#[test]
fn reject_untrusted_signer() {
    let pinned = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let ev = GrantBuilder::install_allow(rid(), non()).build(&attacker); // signed by attacker
    let pk = pinned.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::UntrustedSigner)
    );
}

#[test]
fn reject_event_id_mismatch() {
    let g = TestGuardian::new();
    let mut ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    ev.id = charter_primitives::EventId::from_bytes([0; 32]); // tamper the claimed id
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::EventIdMismatch)
    );
}

#[test]
fn reject_bad_signature() {
    let g = TestGuardian::new();
    let mut ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let mut sig = *ev.sig.as_bytes();
    sig[0] ^= 0x01; // flip a bit; id unchanged so integrity passes, sig fails
    ev.sig = charter_primitives::Sig::from_bytes(sig);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn reject_reqid_mismatch() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let other = ReqId::from_bytes([0xCC; 32]);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &other,
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::ReqIdMismatch)
    );
}

#[test]
fn reject_nonce_mismatch() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let other = Nonce::from_bytes([0xDD; 32]);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &other,
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::NonceMismatch)
    );
}

#[test]
fn reject_op_mismatch() {
    // Confused-deputy / audit-accuracy: a genuine, guardian-signed grant that
    // answers `install.flatpak` must NOT be accepted against a pending record
    // whose op is `time.extend` (a reqId reuse/misroute across two in-flight
    // dialogs). It is rejected with OpMismatch, and — because the op check
    // precedes single-use consume — the reqId is NOT burned, so the genuine
    // grant for the real op can still land.
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g); // op = install.flatpak
    let pk = g.pubkey();
    let s = store();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::TimeExtend, // the pending record's op differs
        now: NOW,
    };
    assert_eq!(verify_grant(&ev, &p, &s), Err(VerifyError::OpMismatch));
    // reqId still consumable => it was not consumed by the rejected grant.
    assert!(s.check_and_consume(&rid(), 1_700_003_600).unwrap());
}

#[test]
fn reject_expired_grant() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .ts(1000)
        .exp(2000)
        .build(&g);
    let pk = g.pubkey();
    // now is far past exp + skew
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: 1_000_000,
    };
    assert_eq!(verify_grant(&ev, &p, &store()), Err(VerifyError::Expired));
}

#[test]
fn reject_future_skew() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .ts(2_000_000)
        .exp(2_003_600)
        .build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: 1_000_000,
    };
    assert_eq!(
        verify_grant(&ev, &p, &store()),
        Err(VerifyError::NotYetValid)
    );
}

#[test]
fn reject_bad_expiry() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .ts(1000)
        .exp(1000)
        .build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: 1000,
    };
    assert_eq!(verify_grant(&ev, &p, &store()), Err(VerifyError::BadExpiry));
}

#[test]
fn reject_replayed_reqid() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let pk = g.pubkey();
    let s = store();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert!(verify_grant(&ev, &p, &s).is_ok());
    assert_eq!(verify_grant(&ev, &p, &s), Err(VerifyError::Replayed)); // replay
}

#[test]
fn replay_within_skew_window_after_purge_still_rejected() {
    // M4: a grant accepted in (exp, exp+SKEW] must stay consumed even after a
    // purge at `now` — retention must cover the whole acceptance window, not
    // just `exp`, or a replay slips through once the entry is purged.
    let g = TestGuardian::new();
    let exp = NOW + 10;
    let ev = GrantBuilder::install_allow(rid(), non())
        .ts(NOW)
        .exp(exp)
        .build(&g);
    let pk = g.pubkey();
    let s = store();
    let now = exp + 100; // exp < now <= exp + SKEW(300): still fresh
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now,
    };
    assert!(
        verify_grant(&ev, &p, &s).is_ok(),
        "a stale-but-in-skew grant is accepted"
    );
    s.purge_expired(now).unwrap();
    assert_eq!(
        verify_grant(&ev, &p, &s),
        Err(VerifyError::Replayed),
        "the reqId must survive the purge so the replay is rejected"
    );
}

#[test]
fn consume_happens_before_grant_returned() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let pk = g.pubkey();
    let s = store();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    let _ = verify_grant(&ev, &p, &s).unwrap();
    // After the grant is returned, the id is already consumed in the store.
    assert!(!s.check_and_consume(&rid(), 1_700_003_600).unwrap());
}

/// A store whose insert always fails — proves verify is fail-CLOSED.
struct FailingStore;
impl ConsumedIdStore for FailingStore {
    fn check_and_consume(&self, _req_id: &ReqId, _exp: u64) -> SysResult<bool> {
        Err(SysError::Io("disk full".into()))
    }
    fn purge_expired(&self, _now: u64) -> SysResult<()> {
        Ok(())
    }
}

#[test]
fn fail_closed_when_consumed_store_insert_fails() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    assert!(matches!(
        verify_grant(&ev, &p, &FailingStore),
        Err(VerifyError::Store(_))
    ));
}

#[test]
fn enact_from_grant_uses_grant_params_not_request() {
    // verify_grant takes NO request params; the approved params come only from
    // the signed grant. A grant carrying ref "org.real.Approved" yields exactly
    // that, regardless of whatever a (hostile) request might have asked for.
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non())
        .params(serde_json::json!({"ref": "org.real.Approved", "remote": "flathub"}))
        .build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    let vg = verify_grant(&ev, &p, &store()).unwrap();
    match vg.allow_params().unwrap() {
        charter_proto::GrantParams::InstallFlatpak(p) => {
            assert_eq!(p.reference, "org.real.Approved")
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn deny_grant_verifies_as_deny() {
    let g = TestGuardian::new();
    let ev = GrantBuilder::install_allow(rid(), non()).deny().build(&g);
    let pk = g.pubkey();
    let p = VerifyParams {
        pinned_guardian: &pk,
        expected_req_id: &rid(),
        expected_nonce: &non(),
        expected_op: OpType::InstallFlatpak,
        now: NOW,
    };
    let vg = verify_grant(&ev, &p, &store()).unwrap();
    assert_eq!(vg.outcome(), &GrantOutcome::Deny);
}
