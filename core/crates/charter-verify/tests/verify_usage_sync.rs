//! Signed usage-sync authentication + replay protection. A hostile relay can
//! delay/drop a sync but never forge one, roll one back, or (because
//! staleness only under-counts the pool) cause a wrongful early lock.

#![cfg(feature = "mock")]

use charter_primitives::kinds;
use charter_verify::test_support::{sign_event, TestGuardian};
use charter_verify::{verify_usage_sync, UsageSyncError};
use serde_json::json;

const NOW: u64 = 1_700_001_000;

fn payload_json(ts: u64, bitmap: Option<&str>) -> String {
    let mut v = json!({
        "v": 1,
        "subject": "cd".repeat(32),
        "ts": ts,
        "dayKey": "2026-06-29",
        "spentElsewhereTodaySecs": 1800,
    });
    if let Some(bm) = bitmap {
        v["elsewhereMinutesToday"] = json!(bm);
    }
    v.to_string()
}

fn build(g: &TestGuardian, ts: u64, bitmap: Option<&str>) -> charter_primitives::NostrEvent {
    sign_event(
        &g.signer,
        kinds::CHARTER_DEVICE_USAGE_SYNC,
        ts,
        vec![charter_primitives::kinds::marker_tag()],
        payload_json(ts, bitmap),
    )
}

#[test]
fn valid_usage_sync_authenticates() {
    let g = TestGuardian::new();
    let ev = build(&g, 500, None);
    let vs = verify_usage_sync(&ev, &g.pubkey(), None, NOW).unwrap();
    assert_eq!(vs.ts(), 500);
    assert_eq!(vs.payload().spent_elsewhere_today_secs, 1800);
}

#[test]
fn valid_bitmap_carries_through() {
    let g = TestGuardian::new();
    let bm = "A".repeat(240);
    let ev = build(&g, 500, Some(&bm));
    let vs = verify_usage_sync(&ev, &g.pubkey(), None, NOW).unwrap();
    assert_eq!(
        vs.payload().elsewhere_minutes_today.as_deref(),
        Some(bm.as_str())
    );
}

#[test]
fn reject_untrusted_signer() {
    let pinned = TestGuardian::new();
    let attacker = TestGuardian::from_seed(0x99);
    let ev = build(&attacker, 500, None);
    assert_eq!(
        verify_usage_sync(&ev, &pinned.pubkey(), None, NOW),
        Err(UsageSyncError::UntrustedSigner)
    );
}

#[test]
fn reject_stale_or_equal_ts() {
    let g = TestGuardian::new();
    let ev = build(&g, 400, None);
    assert_eq!(
        verify_usage_sync(&ev, &g.pubkey(), Some(500), NOW),
        Err(UsageSyncError::StaleTs)
    );
    let ev_eq = build(&g, 500, None);
    assert_eq!(
        verify_usage_sync(&ev_eq, &g.pubkey(), Some(500), NOW),
        Err(UsageSyncError::StaleTs)
    );
}

#[test]
fn reject_bad_signature_and_tampered_content() {
    let g = TestGuardian::new();
    let mut ev = build(&g, 500, None);
    let mut sig = *ev.sig.as_bytes();
    sig[5] ^= 0x10;
    ev.sig = charter_primitives::Sig::from_bytes(sig);
    assert_eq!(
        verify_usage_sync(&ev, &g.pubkey(), None, NOW),
        Err(UsageSyncError::BadSignature)
    );
    // Content tamper after signing: id no longer matches.
    let mut ev2 = build(&g, 500, None);
    ev2.content = payload_json(500, None).replace("1800", "0");
    assert_eq!(
        verify_usage_sync(&ev2, &g.pubkey(), None, NOW),
        Err(UsageSyncError::BadSignature)
    );
}

#[test]
fn reject_kind_mismatch() {
    let g = TestGuardian::new();
    let mut ev = build(&g, 500, None);
    ev.kind = kinds::CHARTER_DEVICE_CLAUSE;
    assert_eq!(
        verify_usage_sync(&ev, &g.pubkey(), None, NOW),
        Err(UsageSyncError::Malformed)
    );
}

#[test]
fn reject_malformed_bitmap() {
    let g = TestGuardian::new();
    for bad in ["short", &"A".repeat(239), &format!("+{}", "A".repeat(239))] {
        let ev = build(&g, 500, Some(bad));
        assert_eq!(
            verify_usage_sync(&ev, &g.pubkey(), None, NOW),
            Err(UsageSyncError::Malformed),
            "accepted bitmap: {bad:.12}"
        );
    }
}
