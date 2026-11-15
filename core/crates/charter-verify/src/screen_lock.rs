//! Verify a guardian SCREEN_LOCK_RESET (kind
//! `CHARTER_DEVICE_SCREEN_LOCK_RESET`, 31122), in two steps, because the
//! device records an outcome for an authentic reset that arrives at the wrong
//! time but records NOTHING for one that is not authentic:
//!
//! 1. [`authenticate_screen_lock_reset`]: kind and payload shape, inner event
//!    id and signature, the pinned guardian as author, THIS machine, and the
//!    pairing epoch (a reset signed before the current pairing is void, as a
//!    RELEASE is).
//! 2. [`check_screen_lock_reset_time`]: the freshness window on the signed
//!    `issuedAt` (an hour back, five minutes forward) and the device's
//!    monotonic floor (strictly above the last accepted reset).
//!
//! Between the two the device looks the reset's `id` up in its own record of
//! recent outcomes, so a relay or local-link re-delivery of an id it has
//! already answered changes nothing.
//!
//! Nothing here formats a payload into an error: every error is a unit
//! variant, so a PIN can never reach a log line through this path.

use charter_primitives::{NostrEvent, PubKey};
use charter_proto::ScreenLockResetPayload;

use crate::event::{event_id, signature_is_valid};

/// Oldest a reset may be, by its signed `issuedAt` against the device clock:
/// one hour. A reset is an "I am standing here now" action, so this is far
/// tighter than a RELEASE's two days.
pub const SCREEN_LOCK_RESET_MAX_AGE_SECS: u64 = 60 * 60;

/// Furthest ahead of the device clock a reset's `issuedAt` may be: five
/// minutes of clock skew.
pub const SCREEN_LOCK_RESET_MAX_AHEAD_SECS: u64 = 5 * 60;

/// Why a reset was not accepted. Every `Err` means "do not reset".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockResetError {
    /// Wrong kind, or a payload that fails its shape rules.
    Malformed,
    /// Event-id integrity or schnorr signature failed.
    BadSignature,
    /// The author is not the pinned guardian.
    UntrustedSigner,
    /// The reset targets a different machine.
    WrongMachine,
    /// Signed before the current pairing: void, as a RELEASE would be.
    BeforePairing,
    /// `issuedAt` is outside `[now - 1h, now + 5min]`.
    OutsideWindow,
    /// `issuedAt` is not strictly above the last accepted reset's.
    NotAboveFloor,
}

impl ScreenLockResetError {
    /// The unit name, for diagnostics that must never carry the payload.
    pub fn as_str(self) -> &'static str {
        match self {
            ScreenLockResetError::Malformed => "malformed",
            ScreenLockResetError::BadSignature => "bad_signature",
            ScreenLockResetError::UntrustedSigner => "untrusted_signer",
            ScreenLockResetError::WrongMachine => "wrong_machine",
            ScreenLockResetError::BeforePairing => "before_pairing",
            ScreenLockResetError::OutsideWindow => "outside_window",
            ScreenLockResetError::NotAboveFloor => "not_above_floor",
        }
    }
}

/// Authenticate a SCREEN_LOCK_RESET for `machine`, signed by
/// `pinned_guardian`, under the pairing established at `paired_at` (`0` =
/// unknown, which admits any time, as for a RELEASE).
///
/// Order: kind/shape → id integrity → signature → pinned author → target
/// machine → pairing epoch. Returns the validated payload. An `Err` here
/// means the device records nothing for this event.
pub fn authenticate_screen_lock_reset(
    event: &NostrEvent,
    pinned_guardian: &PubKey,
    machine: &PubKey,
    paired_at: u64,
) -> Result<ScreenLockResetPayload, ScreenLockResetError> {
    if event.kind != charter_primitives::kinds::CHARTER_DEVICE_SCREEN_LOCK_RESET {
        return Err(ScreenLockResetError::Malformed);
    }
    let payload = ScreenLockResetPayload::from_json(&event.content)
        .map_err(|_| ScreenLockResetError::Malformed)?;
    if event_id(event) != event.id {
        return Err(ScreenLockResetError::BadSignature);
    }
    if !signature_is_valid(event) {
        return Err(ScreenLockResetError::BadSignature);
    }
    if &event.pubkey != pinned_guardian {
        return Err(ScreenLockResetError::UntrustedSigner);
    }
    if &payload.machine != machine {
        return Err(ScreenLockResetError::WrongMachine);
    }
    if payload.issued_at < paired_at {
        return Err(ScreenLockResetError::BeforePairing);
    }
    Ok(payload)
}

/// `true` when `issued_at` is inside the acceptance window at `now`:
/// `now - 3600 <= issued_at <= now + 300`, both edges inclusive.
pub fn screen_lock_reset_in_window(issued_at: u64, now: u64) -> bool {
    issued_at >= now.saturating_sub(SCREEN_LOCK_RESET_MAX_AGE_SECS)
        && issued_at <= now.saturating_add(SCREEN_LOCK_RESET_MAX_AHEAD_SECS)
}

/// The time rules for an AUTHENTICATED reset: the window first
/// ([`ScreenLockResetError::OutsideWindow`], recorded as `rejected_time`),
/// then the monotonic floor ([`ScreenLockResetError::NotAboveFloor`],
/// recorded as `superseded`). `floor` is the `issuedAt` of the last accepted
/// reset (`0` before any).
pub fn check_screen_lock_reset_time(
    payload: &ScreenLockResetPayload,
    now: u64,
    floor: u64,
) -> Result<(), ScreenLockResetError> {
    if !screen_lock_reset_in_window(payload.issued_at, now) {
        return Err(ScreenLockResetError::OutsideWindow);
    }
    if payload.issued_at <= floor {
        return Err(ScreenLockResetError::NotAboveFloor);
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
    const ID: &str = "00112233445566778899aabbccddeeff";

    fn content(id: &str, machine: &PubKey, issued_at: u64, new_pin: &str) -> String {
        ScreenLockResetPayload {
            v: 1,
            id: id.into(),
            machine: *machine,
            issued_at,
            new_pin: new_pin.into(),
        }
        .to_json()
    }

    fn reset_event(signer: &SeedSigner, content: String, issued_at: u64) -> NostrEvent {
        sign_event(
            signer,
            kinds::CHARTER_DEVICE_SCREEN_LOCK_RESET,
            issued_at,
            vec![kinds::marker_tag()],
            content,
        )
    }

    fn machine() -> PubKey {
        PubKey::from_bytes([0x77; 32])
    }

    #[test]
    fn a_guardian_signed_reset_for_this_machine_authenticates() {
        let g = TestGuardian::new();
        let ev = reset_event(&g.signer, content(ID, &machine(), NOW, "123456"), NOW);
        let p = authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), NOW - 10).unwrap();
        assert_eq!(p.id, ID);
        assert_eq!(p.new_pin, "123456");
        assert_eq!(check_screen_lock_reset_time(&p, NOW, 0), Ok(()));
    }

    #[test]
    fn a_reset_that_removes_the_lock_authenticates() {
        let g = TestGuardian::new();
        let ev = reset_event(&g.signer, content(ID, &machine(), NOW, ""), NOW);
        let p = authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0).unwrap();
        assert!(p.removes_screen_lock());
    }

    #[test]
    fn a_reset_from_another_signer_is_untrusted() {
        let g = TestGuardian::new();
        let attacker = SeedSigner::from_seed(0x99);
        let ev = reset_event(&attacker, content(ID, &machine(), NOW, "1234"), NOW);
        assert_eq!(
            authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0),
            Err(ScreenLockResetError::UntrustedSigner)
        );
    }

    #[test]
    fn a_reset_for_another_machine_is_refused() {
        let g = TestGuardian::new();
        let theirs = PubKey::from_bytes([0x88; 32]);
        let ev = reset_event(&g.signer, content(ID, &theirs, NOW, "1234"), NOW);
        assert_eq!(
            authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0),
            Err(ScreenLockResetError::WrongMachine)
        );
    }

    #[test]
    fn a_reset_from_before_the_pairing_is_void() {
        let g = TestGuardian::new();
        let ev = reset_event(&g.signer, content(ID, &machine(), NOW, "1234"), NOW);
        assert_eq!(
            authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), NOW + 1),
            Err(ScreenLockResetError::BeforePairing)
        );
        // Signed in the pairing's own second: admitted, as for a RELEASE.
        assert!(authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), NOW).is_ok());
    }

    #[test]
    fn a_tampered_reset_fails_signature() {
        let g = TestGuardian::new();
        let mut ev = reset_event(&g.signer, content(ID, &machine(), NOW, "1234"), NOW);
        ev.content = content(ID, &machine(), NOW, "0000");
        assert_eq!(
            authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0),
            Err(ScreenLockResetError::BadSignature)
        );
    }

    #[test]
    fn the_wrong_kind_is_malformed() {
        let g = TestGuardian::new();
        let ev = sign_event(
            &g.signer,
            kinds::CHARTER_DEVICE_RELEASE,
            NOW,
            vec![kinds::marker_tag()],
            content(ID, &machine(), NOW, "1234"),
        );
        assert_eq!(
            authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0),
            Err(ScreenLockResetError::Malformed)
        );
    }

    #[test]
    fn bad_ids_and_pins_are_malformed_even_when_signed() {
        let g = TestGuardian::new();
        let cases = [
            content("0011", &machine(), NOW, "1234"),
            content(&ID.to_uppercase(), &machine(), NOW, "1234"),
            content(ID, &machine(), NOW, "123"),
            content(ID, &machine(), NOW, "12345678901234567"),
            content(ID, &machine(), NOW, "12a4"),
        ];
        for c in cases {
            let ev = reset_event(&g.signer, c, NOW);
            assert_eq!(
                authenticate_screen_lock_reset(&ev, &g.pubkey(), &machine(), 0),
                Err(ScreenLockResetError::Malformed)
            );
        }
    }

    #[test]
    fn the_window_is_one_hour_back_and_five_minutes_forward_inclusive() {
        let p = |issued_at| ScreenLockResetPayload {
            v: 1,
            id: ID.into(),
            machine: machine(),
            issued_at,
            new_pin: "1234".into(),
        };
        assert_eq!(check_screen_lock_reset_time(&p(NOW - 3600), NOW, 0), Ok(()));
        assert_eq!(check_screen_lock_reset_time(&p(NOW + 300), NOW, 0), Ok(()));
        assert_eq!(
            check_screen_lock_reset_time(&p(NOW - 3601), NOW, 0),
            Err(ScreenLockResetError::OutsideWindow),
            "stale"
        );
        assert_eq!(
            check_screen_lock_reset_time(&p(NOW + 301), NOW, 0),
            Err(ScreenLockResetError::OutsideWindow),
            "too far in the future"
        );
    }

    #[test]
    fn the_floor_is_strict_and_checked_after_the_window() {
        let p = |issued_at| ScreenLockResetPayload {
            v: 1,
            id: ID.into(),
            machine: machine(),
            issued_at,
            new_pin: "1234".into(),
        };
        assert_eq!(
            check_screen_lock_reset_time(&p(NOW), NOW, NOW),
            Err(ScreenLockResetError::NotAboveFloor),
            "equal to the floor is a replay"
        );
        assert_eq!(check_screen_lock_reset_time(&p(NOW), NOW, NOW - 1), Ok(()));
        assert_eq!(
            check_screen_lock_reset_time(&p(NOW - 4000), NOW, NOW),
            Err(ScreenLockResetError::OutsideWindow),
            "the window answers first"
        );
    }
}
