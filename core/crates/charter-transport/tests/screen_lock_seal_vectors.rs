//! The sealed PIN in the hand-written SCREEN_LOCK_RESET vector
//! (`charter-testkit/vectors/screen_lock/screen_lock_reset_vectors.json`):
//! the ciphertexts were made with nostr-tools' NIP-44 v2, the guardian app's
//! own library, and this side proves the ward's opener against them. Each
//! valid seal opens with the vector's machine key to its `newPin` and
//! re-seals byte-identically from the same ephemeral key and nonce; each
//! `sealFailures` case fails to open with the error it names, so the ward
//! records `failed` for it.

use charter_primitives::PubKey;
use charter_proto::ScreenLockResetPayload;
use charter_transport::{nip44, open_screen_lock_pin, seal_screen_lock_pin, ScreenLockSealError};
use serde_json::Value;

fn load() -> Value {
    charter_testkit::golden::load_json("screen_lock/screen_lock_reset_vectors.json")
}

fn key32(v: &Value) -> [u8; 32] {
    *PubKey::from_hex(v.as_str().unwrap()).unwrap().as_bytes()
}

#[test]
fn the_fixed_keys_are_a_real_key_pair() {
    let keys = &load()["keys"];
    for (sk, pk) in [
        ("machineSecretKey", "machine"),
        ("ephemeralSecretKey", "epk"),
        ("otherMachineSecretKey", "otherMachine"),
    ] {
        assert_eq!(
            charter_crypto::xonly_pubkey(&key32(&keys[sk])).unwrap(),
            key32(&keys[pk]),
            "{pk}"
        );
    }
}

#[test]
fn every_valid_seal_opens_to_its_pin_and_reseals_byte_identically() {
    let doc = load();
    let keys = &doc["keys"];
    let machine_sk = key32(&keys["machineSecretKey"]);
    let ephemeral_sk = key32(&keys["ephemeralSecretKey"]);
    let machine = PubKey::from_hex(keys["machine"].as_str().unwrap()).unwrap();
    for v in doc["vectors"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let pin = v["newPin"].as_str().unwrap();
        let p = ScreenLockResetPayload::from_json(v["content"].as_str().unwrap()).unwrap();
        assert_eq!(p.machine, machine, "{name}");
        let opened = open_screen_lock_pin(&machine_sk, &p)
            .unwrap_or_else(|e| panic!("{name}: would not open: {e:?}"));
        assert_eq!(opened.as_str(), pin, "{name}");
        assert_eq!(
            opened.is_empty(),
            v["removesScreenLock"].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            charter_proto::screen_lock_pin_plaintext(pin).unwrap(),
            v["plaintext"].as_str().unwrap(),
            "{name}: plaintext form"
        );
        let (epk, sealed) =
            seal_screen_lock_pin(pin, &ephemeral_sk, &machine, &key32(&v["nonce"])).unwrap();
        assert_eq!(epk, p.epk, "{name}: epk");
        assert_eq!(sealed, p.new_pin_sealed, "{name}: ciphertext drift");
    }
}

#[test]
fn every_seal_failure_fails_to_open_as_named() {
    let doc = load();
    let keys = &doc["keys"];
    let machine_sk = key32(&keys["machineSecretKey"]);
    let ephemeral_sk = key32(&keys["ephemeralSecretKey"]);
    let failures = doc["sealFailures"].as_array().unwrap();
    assert!(failures
        .iter()
        .any(|f| f["sealedTo"] == keys["otherMachine"]));
    for f in failures {
        let name = f["name"].as_str().unwrap();
        let p = ScreenLockResetPayload::from_json(f["content"].as_str().unwrap()).unwrap();
        let err = open_screen_lock_pin(&machine_sk, &p)
            .map(|_| ())
            .unwrap_err();
        let want = match f["error"].as_str().unwrap() {
            "undecryptable" => ScreenLockSealError::Undecryptable,
            "bad_pin" => ScreenLockSealError::BadPin,
            "bad_key" => ScreenLockSealError::BadKey,
            other => panic!("{name}: unknown error {other}"),
        };
        assert_eq!(err, want, "{name}");
        assert_eq!(err.as_str(), f["error"].as_str().unwrap());
        // Where the vector says how it was made, the bytes are reproducible.
        if let (Some(plain), Some(to)) = (f["plaintext"].as_str(), f["sealedTo"].as_str()) {
            let key =
                nip44::conversation_key(&ephemeral_sk, PubKey::from_hex(to).unwrap().as_bytes())
                    .unwrap();
            assert_eq!(
                nip44::encrypt(plain, &key, &key32(&f["nonce"])).unwrap(),
                p.new_pin_sealed,
                "{name}: ciphertext drift"
            );
        }
    }
}
