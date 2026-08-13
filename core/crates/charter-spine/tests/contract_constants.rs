//! §5.6 cross-stack constant parity — the Rust half.
//!
//! These magic numbers are shared across the wire boundary: the guardian PWA
//! (TypeScript) and the wardens (Rust) MUST agree on every one, or valid
//! messages get silently rejected (a wrap outside the jitter window, a grant
//! outside the skew window, a kind that doesn't match). The mirror test lives
//! at `apps/charter-app/src/wire/contractConstants.test.ts`; both assert the
//! SAME literals, so a change on either stack that isn't matched on the other
//! turns a silent interop failure into a loud red test.
//!
//! Source of truth: `spec/contract.md` §5.6.

use charter_primitives::kinds;
use charter_proto::params::{MAX_EXTEND_MINUTES, MAX_REASON_LEN};
use charter_transport::nip59::MAX_JITTER_SECS;
use charter_verify::FRESHNESS_SKEW_SECS;

#[test]
fn wrap_jitter_is_two_days() {
    assert_eq!(MAX_JITTER_SECS, 2 * 24 * 60 * 60);
    assert_eq!(MAX_JITTER_SECS, 172_800);
}

#[test]
fn grant_freshness_skew_is_300s() {
    assert_eq!(FRESHNESS_SKEW_SECS, 300);
}

#[test]
fn time_extend_bounds() {
    assert_eq!(MAX_EXTEND_MINUTES, 1440);
    assert_eq!(MAX_REASON_LEN, 280);
}

#[test]
fn event_kinds() {
    assert_eq!(kinds::GIFT_WRAP, 1059);
    assert_eq!(kinds::SEAL, 13);
    assert_eq!(kinds::CHARTER_DEVICE_REQUEST, 31111);
    assert_eq!(kinds::CHARTER_DEVICE_GRANT, 31112);
    assert_eq!(kinds::CHARTER_DEVICE_CLAUSE, 31113);
    assert_eq!(kinds::CHARTER_DEVICE_STATUS, 31114);
    assert_eq!(kinds::CHARTER_DEVICE_USAGE_SYNC, 31115);
    assert_eq!(kinds::CHARTER_DEVICE_AUDIT, 31000);
    assert_eq!(kinds::CHARTER_DEVICE_RELEASE, 31116);
    assert_eq!(kinds::CHARTER_CURATOR_WEB_LIST, 30100);
}

#[test]
fn marker_tag() {
    assert_eq!(
        kinds::marker_tag(),
        vec!["t".to_string(), "charter-device".to_string()]
    );
}
