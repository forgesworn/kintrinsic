//! Guardian-side JNI for the MyCharter carrier APK. The Rust core owns
//! unwrap/classify (single-sourced with the ward warden's crypto); Kotlin owns
//! the websocket, storage, and notifications.

pub mod classify;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jint, jlong, jstring};
use jni::JNIEnv;
use zeroize::Zeroize;

/// Bump on any breaking change to this surface; Kotlin refuses a mismatch.
///
/// **2 —** the guardian secret is now a `ByteArray`, not a hex `String` (S12,
/// review 2026-08-07). A JVM `String` is immutable and interned into the heap,
/// so every classify call — and the carrier classifies every wrap the relay
/// delivers — minted another copy of the family's root signing key that
/// nothing could erase and only the GC would eventually reclaim. A byte array
/// can be wiped, by both sides, the moment the call returns.
pub const ABI_VERSION: jint = 2;

fn out(env: &JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Copy a 32-byte secret out of a JVM byte array, wiping the intermediate.
///
/// `convert_byte_array` allocates a `Vec<u8>` on the Rust side; without the
/// explicit zeroize that copy is freed intact and sits in a reusable heap page
/// (S12). The caller's array is Kotlin's to wipe — and it does.
fn take_sk(env: &JNIEnv, arr: &JByteArray) -> Option<zeroize::Zeroizing<[u8; 32]>> {
    let mut bytes = env.convert_byte_array(arr).ok()?;
    let out = (bytes.len() == 32).then(|| {
        let mut sk = [0u8; 32];
        sk.copy_from_slice(&bytes);
        zeroize::Zeroizing::new(sk)
    });
    bytes.zeroize();
    out
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianAbiVersion(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    ABI_VERSION
}

/// Classify one wrap. Total function: any failure returns a `{"type":"drop"}`
/// JSON, and a panic is caught (never unwinds across FFI). Worker thread only
/// (crypto, ~ms — keep off the main thread by the ward-app rule).
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianClassifyWrap(
    mut env: JNIEnv,
    _class: JClass,
    wrap_event_json: JString,
    guardian_sk: JByteArray,
    now_unix: jlong,
) -> jstring {
    let wrap: String = match env.get_string(&wrap_event_json) {
        Ok(s) => s.into(),
        Err(_) => return out(&env, r#"{"type":"drop","reason":"bad jstring"}"#),
    };
    let Some(sk) = take_sk(&env, &guardian_sk) else {
        return out(&env, r#"{"type":"drop","reason":"bad guardian sk"}"#);
    };
    let res =
        std::panic::catch_unwind(|| classify::classify_wrap(&wrap, &sk, now_unix.max(0) as u64))
            .unwrap_or_else(|_| r#"{"type":"drop","reason":"internal panic"}"#.to_string());
    // `sk` is Zeroizing: wiped here, at the end of the call, rather than left
    // in a freed page for as long as the process lives.
    out(&env, &res)
}
