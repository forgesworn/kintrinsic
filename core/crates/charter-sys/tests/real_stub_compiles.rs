//! Proves the `real` feature compiles and `RealSystem` wires every port. The
//! machine signer is now a real impl (verified by the `#[cfg(feature = "real")]`
//! tests in `signer.rs`); here we pin its **fail-safe** contract through the
//! aggregate: an unprovisioned host (no key file) yields a signer that errors on
//! `sign` and reports a zeroed pubkey rather than panicking — the broker boots
//! and degrades closed. The remaining OS-effect ports finalize on the VM.

#![cfg(feature = "real")]

use charter_sys::signer::{MachineSigner, RealMachineSigner};
use charter_sys::{RealSystem, SystemLayer};

#[test]
fn real_system_machine_signer_is_fail_safe_when_unprovisioned() {
    // RealSystem wires the real machine signer (compile + wiring proof).
    let sys = RealSystem::default();
    let _ = sys.machine().pubkey();

    // An explicitly-absent key path is deterministically unprovisioned,
    // regardless of host state: fail-safe, never a panic.
    let path = std::env::temp_dir().join(format!("charter-absent-key-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let signer = RealMachineSigner::with_key_path(&path);
    assert!(!signer.is_provisioned());
    assert_eq!(signer.pubkey().as_bytes(), &[0u8; 32]);
    assert!(signer.sign(&[0u8; 32]).is_err());
}
