//! `verify_release` — authenticate a parent-gated device RELEASE (kind
//! `CHARTER_DEVICE_RELEASE`, 31116) against the pinned guardian's inner NIP-01
//! signature AND author, confirm it targets THIS machine, and bound its
//! freshness. Unlike a clause, a release has no monotonic-issuedAt state: after
//! it applies the pairing is gone, so a replayed release finds no pinned
//! guardian to match and is dropped; the freshness bound stops a long-delayed
//! replay from unpairing a device that has since re-paired to the SAME guardian.

use charter_primitives::{NostrEvent, PubKey};
use charter_proto::ReleasePayload;

use crate::event::{event_id, signature_is_valid};

/// Why a release was rejected. Every `Err` means "do not release".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseError {
    /// Wrong kind or unparseable payload.
    Malformed,
    /// Event-id integrity or schnorr signature failed.
    BadSignature,
    /// The author is not the pinned guardian.
    UntrustedSigner,
    /// The release targets a different machine.
    WrongMachine,
    /// Outside the freshness window (stale replay / future-dated).
    Stale,
}

/// Freshness bound for a release, matching the NIP-59 wrap jitter (2 days).
/// Wide enough for a lagging device clock, tight enough that a week-old
/// replayed release can't unpair a since-re-paired device.
pub const RELEASE_FRESHNESS_SECS: u64 = 2 * 24 * 60 * 60;

/// Authenticate a RELEASE event for `machine`, signed by `pinned_guardian`.
/// Order: kind/shape → id integrity → signature → pinned author → target
/// machine → freshness.
pub fn verify_release(
    event: &NostrEvent,
    pinned_guardian: &PubKey,
    machine: &PubKey,
    now: u64,
) -> Result<(), ReleaseError> {
    if event.kind != charter_primitives::kinds::CHARTER_DEVICE_RELEASE {
        return Err(ReleaseError::Malformed);
    }
    let payload = ReleasePayload::from_json(&event.content).map_err(|_| ReleaseError::Malformed)?;

    if event_id(event) != event.id {
        return Err(ReleaseError::BadSignature);
    }
    if !signature_is_valid(event) {
        return Err(ReleaseError::BadSignature);
    }
    if &event.pubkey != pinned_guardian {
        return Err(ReleaseError::UntrustedSigner);
    }
    if &payload.machine != machine {
        return Err(ReleaseError::WrongMachine);
    }
    // Freshness: |now - issuedAt| within the window.
    let delta = now.abs_diff(payload.issued_at);
    if delta > RELEASE_FRESHNESS_SECS {
        return Err(ReleaseError::Stale);
    }
    Ok(())
}

#[cfg(all(test, feature = "mock"))]
mod verify_tests {
    use super::*;
    use crate::test_support::{sign_event, TestGuardian};
    use charter_primitives::kinds;
    use charter_sys::signer::SeedSigner;

    const NOW: u64 = 1_700_000_000;

    fn release_event(signer: &SeedSigner, machine: &PubKey, issued_at: u64) -> NostrEvent {
        let payload = ReleasePayload {
            v: 1,
            machine: *machine,
            issued_at,
        };
        sign_event(
            signer,
            kinds::CHARTER_DEVICE_RELEASE,
            issued_at,
            vec![kinds::marker_tag()],
            payload.to_json(),
        )
    }

    #[test]
    fn a_guardian_signed_release_for_this_machine_verifies() {
        let guardian = TestGuardian::new();
        let machine = PubKey::from_bytes([0x77; 32]);
        let ev = release_event(&guardian.signer, &machine, NOW);
        assert!(verify_release(&ev, &guardian.pubkey(), &machine, NOW).is_ok());
    }

    #[test]
    fn a_release_from_a_different_signer_is_untrusted() {
        let guardian = TestGuardian::new();
        let attacker = SeedSigner::from_seed(0x99);
        let machine = PubKey::from_bytes([0x77; 32]);
        let ev = release_event(&attacker, &machine, NOW);
        // Signed correctly, but NOT by the pinned guardian → refused (this is
        // what makes it parent-gated: the child can't forge it).
        assert_eq!(
            verify_release(&ev, &guardian.pubkey(), &machine, NOW),
            Err(ReleaseError::UntrustedSigner)
        );
    }

    #[test]
    fn a_release_targeting_another_machine_is_refused() {
        let guardian = TestGuardian::new();
        let ours = PubKey::from_bytes([0x77; 32]);
        let theirs = PubKey::from_bytes([0x88; 32]);
        let ev = release_event(&guardian.signer, &theirs, NOW);
        assert_eq!(
            verify_release(&ev, &guardian.pubkey(), &ours, NOW),
            Err(ReleaseError::WrongMachine)
        );
    }

    #[test]
    fn a_stale_or_future_release_is_refused() {
        let guardian = TestGuardian::new();
        let machine = PubKey::from_bytes([0x77; 32]);
        let old = release_event(&guardian.signer, &machine, NOW);
        // A week later, the same release is outside the freshness window.
        assert_eq!(
            verify_release(&old, &guardian.pubkey(), &machine, NOW + 7 * 24 * 3600),
            Err(ReleaseError::Stale)
        );
    }

    #[test]
    fn a_tampered_release_fails_signature() {
        let guardian = TestGuardian::new();
        let machine = PubKey::from_bytes([0x77; 32]);
        let mut ev = release_event(&guardian.signer, &machine, NOW);
        // Flip the payload after signing — id/sig no longer match.
        ev.content = ReleasePayload {
            v: 1,
            machine,
            issued_at: NOW + 1,
        }
        .to_json();
        assert_eq!(
            verify_release(&ev, &guardian.pubkey(), &machine, NOW),
            Err(ReleaseError::BadSignature)
        );
    }
}
