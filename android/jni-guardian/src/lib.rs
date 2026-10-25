//! Guardian-side JNI for the MyCharter carrier APK. The Rust core owns
//! unwrap/classify (single-sourced with the ward warden's crypto); Kotlin owns
//! the websocket, storage, and notifications.

pub mod classify;
pub mod link;

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};
use zeroize::Zeroize;

/// Bump on any breaking change to this surface; Kotlin refuses a mismatch.
///
/// **2 —** the guardian secret is now a `ByteArray`, not a hex `String` (S12,
/// review 2026-08-07). A JVM `String` is immutable and interned into the heap,
/// so every classify call — and the carrier classifies every wrap the relay
/// delivers — minted another copy of the family's root signing key that
/// nothing could erase and only the GC would eventually reclaim. A byte array
/// can be wiped, by both sides, the moment the call returns.
///
/// **3 —** the local link (v1): `guardianLink*` start/stop the home-network
/// listener and carry wraps over it (docs/local-network-design.md).
pub const ABI_VERSION: jint = 3;

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
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianRelayAuth(
    mut env: JNIEnv,
    _class: JClass,
    relay: JString,
    challenge: JString,
    guardian_sk: JByteArray,
    now_unix: jlong,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Option<String> {
        let relay: String = env.get_string(&relay).ok()?.into();
        let challenge: String = env.get_string(&challenge).ok()?.into();
        if !relay.starts_with("wss://")
            || challenge.is_empty()
            || challenge.len() > 4096
            || now_unix < 0
        {
            return None;
        }
        let sk = take_sk(&env, &guardian_sk)?;
        let proof = charter_transport::nip59::sign_event_raw(
            &sk,
            22242,
            now_unix as u64,
            vec![
                vec!["relay".into(), relay],
                vec!["challenge".into(), challenge],
            ],
            String::new(),
        )
        .ok()?;
        serde_json::to_string(&proof).ok()
    }));
    out(
        &env,
        &result
            .ok()
            .flatten()
            .unwrap_or_else(|| "{\"error\":\"Could not authenticate to the relay.\"}".into()),
    )
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

// ---- local link (ABI 3) ----------------------------------------------------
//
// Every entry point returns a JSON string and never unwinds across FFI: a
// panic, or a string that cannot cross the boundary, becomes the fallback.
// The shapes and decisions live in `link.rs`.

fn jstr(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

fn guarded(env: &JNIEnv, fallback: &str, f: impl FnOnce() -> Option<String>) -> jstring {
    let res = catch_unwind(AssertUnwindSafe(f)).ok().flatten();
    out(env, res.as_deref().unwrap_or(fallback))
}

const ERR: &str = r#"{"ok":false,"error":"internal"}"#;

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkStart(
    mut env: JNIEnv,
    _class: JClass,
    state_dir: JString,
    guardian_sk: JByteArray,
    allowed_json: JString,
    preferred_port: jint,
) -> jstring {
    // The key is copied into `Zeroizing` and the JVM-side copy is wiped by
    // the Kotlin caller.
    let sk = take_sk(&env, &guardian_sk);
    let dir = jstr(&mut env, &state_dir);
    let allowed = jstr(&mut env, &allowed_json);
    guarded(&env, ERR, move || {
        let Some(sk) = sk else {
            return Some(link::error_json("bad guardian key"));
        };
        Some(link::start(&dir?, sk, &allowed?, preferred_port))
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkStop(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    guarded(&env, ERR, || Some(link::stop()))
}

/// Accept wards (`true`) only while on a home network; `false` refuses new
/// connections and closes sessions, keeping the queue and inbox.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkAccepting(
    env: JNIEnv,
    _class: JClass,
    on: jboolean,
) -> jstring {
    guarded(&env, ERR, || Some(link::accepting(on != 0)))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkAllow(
    mut env: JNIEnv,
    _class: JClass,
    allowed_json: JString,
) -> jstring {
    let allowed = jstr(&mut env, &allowed_json);
    guarded(&env, ERR, || Some(link::allow(&allowed?)))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkSend(
    mut env: JNIEnv,
    _class: JClass,
    recipient_hex: JString,
    wrap_json: JString,
) -> jstring {
    let recipient = jstr(&mut env, &recipient_hex);
    let wrap = jstr(&mut env, &wrap_json);
    guarded(&env, r#"{"state":"rejected","reason":"internal"}"#, || {
        Some(link::send(&recipient?, &wrap?))
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkNext(
    env: JNIEnv,
    _class: JClass,
    timeout_ms: jint,
) -> jstring {
    guarded(&env, "[]", || Some(link::next(timeout_ms)))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkInbox(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    guarded(&env, "[]", || Some(link::inbox()))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkConsume(
    mut env: JNIEnv,
    _class: JClass,
    ids_json: JString,
) -> jstring {
    let ids = jstr(&mut env, &ids_json);
    guarded(&env, ERR, || Some(link::consume(&ids?)))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkDelivery(
    mut env: JNIEnv,
    _class: JClass,
    ids_json: JString,
) -> jstring {
    let ids = jstr(&mut env, &ids_json);
    guarded(&env, "{}", || Some(link::delivery(&ids?)))
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_mycharter_native_GuardianNative_guardianLinkSessions(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    guarded(&env, "[]", || Some(link::sessions()))
}
