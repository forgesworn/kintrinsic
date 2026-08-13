# MyCharter Carrier APK Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native Android app for the **guardian's** phone that wraps the MyCharter PWA in a WebView and adds the one thing a PWA cannot do: an always-on foreground service holding a live relay subscription, so a ward's ask reliably lights the guardian's screen within seconds — no Google/FCM.

**Architecture:** Three thin layers, maximum reuse. (1) A new lean Rust crate `android/jni-guardian` exposes ONE JNI function — classify a gift-wrap JSON with the guardian secret → request/status/other — reusing `charter-transport::nip59`, `charter-proto`, `charter-primitives` (the same proven stack as the ward app). (2) A new `:carrier` Gradle module (applicationId `org.forgesworn.mycharter`) with a WebView shell over `https://charter.mysignet.app`, a JS bridge that receives the guardian key from the web app, a foreground service holding OkHttp websockets to the relays, and a full-screen approval alert. (3) A small PWA addition: when `window.CharterCarrier` exists, hand it the key + relays. Decisions and GRANT signing stay 100% in the existing web app — the native layer only classifies and notifies. Per spec D2 (vault discipline) the carrier does NOT grow custody features; the key copy lives in app-private storage exactly as the WebView's localStorage copy does.

**Tech Stack:** Rust (jni 0.21, charter core crates), Kotlin (AGP, compileSdk 36, minSdk 29 for carrier), OkHttp 4.12.0 websockets, React/TS (PWA bridge), vitest, JUnit4 host tests, cargo-ndk.

## Global Constraints

- Android/Rust builds need `source ~/Android/env.sh` first (SDK/NDK/cargo-ndk). `/tmp` is noexec — never build there.
- Android/Kotlin/JNI are NOT in CI — every gate is run locally and its output shown.
- Never call JNI from the JVM main thread (worker threads only) — matches the ward-app rule.
- The shipped `.so` must never carry the `mock` cargo feature (release gate greps the feature graph), and must be 16 KB page-aligned (Android 15/GrapheneOS).
- Wire constants (from `core/crates/charter-primitives/src/kinds.rs`): GIFT_WRAP=1059, CHARTER_DEVICE_REQUEST=31111, CHARTER_DEVICE_GRANT=31112, CHARTER_DEVICE_CLAUSE=31113, CHARTER_DEVICE_STATUS=31114. The carrier notifies ONLY on kind-31111 rumors.
- Default relay: `wss://relay.trotters.cc` (must arrive via the provision payload, never hardcoded in Kotlin).
- Guardian key: the PWA persists it as `nsec…` in localStorage key `charter.guardian.key.v1`; the bridge passes the 64-char lowercase hex form.
- Ward-app naming stays untouched: nothing in `android/app` or `android/jni` changes behaviour (Task 9 adds an example binary to `android/jni` only).
- Wardship lexicon in copy: guardian/ward/charter/clause (never "parental controls").
- PWA gates from `apps/charter-app/`: `npm test` (vitest) must stay green.
- Commit after every task; conventional-commit messages.

---

### Task 1: `charter-guardian-jni` crate — pure classify logic

**Files:**
- Create: `android/jni-guardian/Cargo.toml`
- Create: `android/jni-guardian/src/lib.rs`
- Create: `android/jni-guardian/src/classify.rs`

**Interfaces:**
- Produces: `classify::classify_wrap(wrap_json: &str, guardian_sk: &[u8; 32], now_unix: u64) -> String` — always returns a JSON string, one of:
  - `{"type":"request","reqId":"<hex64>","op":"time.extend","machine":"<hex64>","subject":"<hex64>","ts":<u64>,"params":{…}}`
  - `{"type":"status","machine":"<hex64>"}`
  - `{"type":"other","kind":<u16>}`
  - `{"type":"drop","reason":"<short text>"}` (bad JSON, unwrap failure, stale, spoofed machine)

- [ ] **Step 1: Create the crate manifest**

`android/jni-guardian/Cargo.toml`:

```toml
[package]
name = "charter-guardian-jni"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
name = "charter_guardian_jni"
crate-type = ["cdylib", "rlib"]

[dependencies]
jni = "0.21"
serde_json = "1"

# The shared wire/crypto core — same crates the ward jni uses. No charter-sys:
# the carrier's websocket lives in Kotlin; Rust only unwraps and classifies.
charter-primitives = { path = "../../core/crates/charter-primitives" }
charter-proto = { path = "../../core/crates/charter-proto" }
charter-transport = { path = "../../core/crates/charter-transport", default-features = false }

[dev-dependencies]
# Fixture building (TestGuardian, sign_event) — mock feature in tests ONLY;
# feature unification never reaches the release .so (dev-deps don't ship).
charter-verify = { path = "../../core/crates/charter-verify", features = ["mock"] }
getrandom = "0.2"

# Standalone workspace while core/ stays a sibling; joins nothing.
[workspace]
```

- [ ] **Step 2: Write the failing tests**

`android/jni-guardian/src/classify.rs` (tests first — the module body in Step 4 goes above them):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::{kinds, Nonce, PubKey, ReqId};
    use charter_proto::{OpType, RequestPayload};
    use charter_transport::nip59::{self, Rumor, WrapRandomness};
    use charter_verify::test_support::{sign_event, TestGuardian};

    fn rand32() -> [u8; 32] {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).expect("entropy");
        b
    }

    /// Wrap a signed event from `sender` (machine) to `recipient_pk`, as JSON.
    fn wrap_json(
        ev: &charter_primitives::NostrEvent,
        sender_sk: &[u8; 32],
        recipient_pk: &PubKey,
        now: u64,
    ) -> String {
        let wrap = nip59::wrap(
            &Rumor::from_signed_event(ev),
            sender_sk,
            recipient_pk.as_bytes(),
            &WrapRandomness {
                ephemeral_secret: rand32(),
                seal_nonce: rand32(),
                wrap_nonce: rand32(),
                seal_created_at: now,
                wrap_created_at: now,
            },
        )
        .expect("wrap");
        serde_json::to_string(&wrap).expect("wrap json")
    }

    /// A signed kind-31111 REQUEST from `machine`, wrapped to `guardian`.
    fn request_wrap(guardian: &TestGuardian, machine: &TestGuardian, now: u64) -> String {
        let payload = RequestPayload {
            v: 1,
            op: OpType::TimeExtend,
            req_id: ReqId::from_bytes([7; 32]),
            nonce: Nonce::from_bytes([8; 32]),
            subject: PubKey::from_bytes([9; 32]),
            machine: machine.pubkey(),
            ts: now,
            params: serde_json::json!({"minutesRequested": 30, "limitHit": "budget"}),
        };
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_REQUEST,
            now,
            vec![kinds::marker_tag()],
            payload.to_json(),
        );
        wrap_json(&ev, machine.signer_secret(), &guardian.pubkey(), now)
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn classifies_a_request_wrap() {
        let guardian = TestGuardian::new();
        let machine = TestGuardian::from_seed(0x44);
        let json = request_wrap(&guardian, &machine, NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, guardian.signer_secret(), NOW)).unwrap();
        assert_eq!(out["type"], "request");
        assert_eq!(out["op"], "time.extend");
        assert_eq!(out["machine"], machine.pubkey().to_hex());
        assert_eq!(out["params"]["minutesRequested"], 30);
        assert_eq!(out["reqId"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn classifies_a_status_wrap_without_payload_leak() {
        let guardian = TestGuardian::new();
        let machine = TestGuardian::from_seed(0x44);
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_STATUS,
            NOW,
            vec![kinds::marker_tag()],
            r#"{"v":1}"#.to_string(),
        );
        let json = wrap_json(&ev, machine.signer_secret(), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, guardian.signer_secret(), NOW)).unwrap();
        assert_eq!(out["type"], "status");
        assert_eq!(out["machine"], machine.pubkey().to_hex());
        assert!(out.get("params").is_none(), "status must not carry content");
    }

    #[test]
    fn drops_a_wrap_for_someone_else() {
        let guardian = TestGuardian::new();
        let stranger = TestGuardian::from_seed(0x55);
        let machine = TestGuardian::from_seed(0x44);
        // Wrapped to the STRANGER — our guardian key cannot unwrap it.
        let json = request_wrap(&stranger, &machine, NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, guardian.signer_secret(), NOW)).unwrap();
        assert_eq!(out["type"], "drop");
    }

    #[test]
    fn drops_a_spoofed_machine_claim() {
        // Payload claims machine=X but the seal author is Y: refuse attribution
        // (mirror of the PWA's unwrapRequest spoof guard).
        let guardian = TestGuardian::new();
        let real_author = TestGuardian::from_seed(0x44);
        let claimed = TestGuardian::from_seed(0x66);
        let payload = RequestPayload {
            v: 1,
            op: OpType::TimeExtend,
            req_id: ReqId::from_bytes([7; 32]),
            nonce: Nonce::from_bytes([8; 32]),
            subject: PubKey::from_bytes([9; 32]),
            machine: claimed.pubkey(), // lie
            ts: NOW,
            params: serde_json::json!({}),
        };
        let ev = sign_event(
            &real_author.signer,
            kinds::CHARTER_DEVICE_REQUEST,
            NOW,
            vec![kinds::marker_tag()],
            payload.to_json(),
        );
        let json = wrap_json(&ev, real_author.signer_secret(), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, guardian.signer_secret(), NOW)).unwrap();
        assert_eq!(out["type"], "drop");
    }

    #[test]
    fn other_kinds_report_kind_only() {
        let guardian = TestGuardian::new();
        let machine = TestGuardian::from_seed(0x44);
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_GRANT,
            NOW,
            vec![kinds::marker_tag()],
            r#"{"v":1}"#.to_string(),
        );
        let json = wrap_json(&ev, machine.signer_secret(), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, guardian.signer_secret(), NOW)).unwrap();
        assert_eq!(out["type"], "other");
        assert_eq!(out["kind"], u16::from(kinds::CHARTER_DEVICE_GRANT));
    }

    #[test]
    fn garbage_json_is_a_drop_not_a_panic() {
        let guardian = TestGuardian::new();
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap("not json at all", guardian.signer_secret(), NOW))
                .unwrap();
        assert_eq!(out["type"], "drop");
    }
}
```

NOTE for the implementer: `TestGuardian` lives in `charter_verify::test_support`. Check its actual accessor for the secret — `live_guardian.rs` constructs `TestGuardian { signer: SeedSigner::from_secret(sk) }`, so if there is no `signer_secret()` method, derive the fixture the same way: keep the raw `[u8; 32]` seed in the test, build `SeedSigner::from_secret`, and pass the raw seed to `wrap_json`/`classify_wrap` directly. Adjust the helper signatures accordingly — the assertions are the contract, not the helper shapes.

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd ~/charter/android/jni-guardian
cargo test 2>&1 | tail -20
```

Expected: compile error — `classify_wrap` not defined.

- [ ] **Step 4: Implement `classify_wrap`**

Top of `android/jni-guardian/src/classify.rs` (above the tests):

```rust
//! Classify one gift-wrap addressed to the guardian: is it a ward's ask
//! (notify!), a status heartbeat (ignore), or something else? Pure logic —
//! no IO, no state — so the whole carrier brain is host-testable.

use charter_primitives::{kinds, NostrEvent};
use charter_proto::RequestPayload;
use charter_transport::nip59;

fn drop_json(reason: &str) -> String {
    serde_json::json!({"type": "drop", "reason": reason}).to_string()
}

/// Classify a wrap event (JSON) using the guardian secret. Total: every input
/// maps to a JSON verdict; malformed/foreign/stale input is a "drop", never an
/// error the service has to special-case.
pub fn classify_wrap(wrap_json: &str, guardian_sk: &[u8; 32], now_unix: u64) -> String {
    let wrap: NostrEvent = match serde_json::from_str(wrap_json) {
        Ok(e) => e,
        Err(_) => return drop_json("bad wrap json"),
    };
    let (rumor, author) =
        match nip59::unwrap_with_author(&wrap, guardian_sk, now_unix, nip59::MAX_JITTER_SECS) {
            Ok(pair) => pair,
            Err(_) => return drop_json("unwrap failed"),
        };
    match rumor.kind {
        k if k == kinds::CHARTER_DEVICE_REQUEST => {
            let req = match RequestPayload::from_json(&rumor.content) {
                Ok(r) => r,
                Err(_) => return drop_json("bad request payload"),
            };
            // Attribution guard: the payload's machine claim must be the seal
            // author, or a compromised relay/device could impersonate another.
            if req.machine != author {
                return drop_json("machine/author mismatch");
            }
            serde_json::json!({
                "type": "request",
                "reqId": req.req_id.to_hex(),
                "op": req.op,
                "machine": req.machine.to_hex(),
                "subject": req.subject.to_hex(),
                "ts": req.ts,
                "params": req.params,
            })
            .to_string()
        }
        k if k == kinds::CHARTER_DEVICE_STATUS => {
            // Deliberately payload-free: the service needs "a heartbeat came
            // from this machine", never the content.
            serde_json::json!({"type": "status", "machine": author.to_hex()}).to_string()
        }
        k => serde_json::json!({"type": "other", "kind": k}).to_string(),
    }
}
```

`android/jni-guardian/src/lib.rs` (JNI comes in Task 2 — for now just the module):

```rust
//! Guardian-side JNI for the MyCharter carrier APK. The Rust core owns
//! unwrap/classify (single-sourced with the ward warden's crypto); Kotlin owns
//! the websocket, storage, and notifications.

pub mod classify;
```

If `req.op` does not serialize to the wire string (`"time.extend"`) via `serde_json::json!`, check `charter_proto::OpType`'s serde attributes and serialize with `serde_json::to_value(&req.op)` — the test asserts the exact string.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test 2>&1 | tail -10
```

Expected: `test result: ok. 6 passed`

- [ ] **Step 6: Commit**

```bash
cd ~/charter
git add android/jni-guardian
git commit -m "feat(carrier): charter-guardian-jni crate — pure gift-wrap classify core"
```

---

### Task 2: JNI surface + `.so` build script

**Files:**
- Modify: `android/jni-guardian/src/lib.rs`
- Create: `android/scripts/build-jni-guardian.sh`

**Interfaces:**
- Consumes: `classify::classify_wrap` (Task 1).
- Produces: JNI symbols on `libcharter_guardian_jni.so` for Kotlin `object GuardianNative` (Task 3's package `org.forgesworn.mycharter.native`):
  - `guardianAbiVersion(): Int` (== 1)
  - `guardianClassifyWrap(wrapEventJson: String, guardianSkHex: String, nowUnix: Long): String`

- [ ] **Step 1: Add the JNI entry points to `lib.rs`**

```rust
//! Guardian-side JNI for the MyCharter carrier APK. The Rust core owns
//! unwrap/classify (single-sourced with the ward warden's crypto); Kotlin owns
//! the websocket, storage, and notifications.

pub mod classify;

use jni::objects::{JClass, JString};
use jni::sys::{jint, jlong, jstring};
use jni::JNIEnv;

/// Bump on any breaking change to this surface; Kotlin refuses a mismatch.
pub const ABI_VERSION: jint = 1;

fn out(env: &JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn parse_sk_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut b = [0u8; 32];
    for i in 0..32 {
        b[i] = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(b)
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
    guardian_sk_hex: JString,
    now_unix: jlong,
) -> jstring {
    let wrap: String = match env.get_string(&wrap_event_json) {
        Ok(s) => s.into(),
        Err(_) => return out(&env, r#"{"type":"drop","reason":"bad jstring"}"#),
    };
    let sk_hex: String = match env.get_string(&guardian_sk_hex) {
        Ok(s) => s.into(),
        Err(_) => return out(&env, r#"{"type":"drop","reason":"bad jstring"}"#),
    };
    let Some(sk) = parse_sk_hex(&sk_hex) else {
        return out(&env, r#"{"type":"drop","reason":"bad guardian sk hex"}"#);
    };
    let res = std::panic::catch_unwind(|| {
        classify::classify_wrap(&wrap, &sk, now_unix.max(0) as u64)
    })
    .unwrap_or_else(|_| r#"{"type":"drop","reason":"internal panic"}"#.to_string());
    out(&env, &res)
}
```

- [ ] **Step 2: Host tests still pass**

```bash
cd ~/charter/android/jni-guardian && cargo test 2>&1 | tail -5
```

Expected: `6 passed`.

- [ ] **Step 3: Create the build script**

`android/scripts/build-jni-guardian.sh` (mirror of `build-jni.sh`, no features to juggle — the crate has no `mock`/`real-relay` features at all):

```bash
#!/usr/bin/env bash
# Build libcharter_guardian_jni.so for both shipping ABIs into
# carrier/src/main/jniLibs. Requires: `source ~/Android/env.sh` + cargo-ndk.
set -euo pipefail
cd "$(dirname "$0")/../jni-guardian"

PROFILE="${1:-release}"
case "$PROFILE" in
  release) cargo ndk -t arm64-v8a -t x86_64 -o ../carrier/src/main/jniLibs build --release ;;
  debug)   cargo ndk -t arm64-v8a -t x86_64 -o ../carrier/src/main/jniLibs build ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

# 16 KB page alignment (Android 15 / GrapheneOS requirement) — same gate as
# the ward .so.
for so in ../carrier/src/main/jniLibs/*/libcharter_guardian_jni.so; do
  align=$("$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf -l "$so" \
    | awk '/LOAD/ {print $NF}' | sort -u | head -1)
  if [ "$align" != "0x4000" ]; then
    echo "FATAL: $so LOAD align $align != 0x4000 (16 KB)" >&2
    exit 1
  fi
done
echo "OK: guardian jniLibs built ($PROFILE), 16 KB aligned."
```

- [ ] **Step 4: Build the `.so` and verify the gate**

```bash
chmod +x android/scripts/build-jni-guardian.sh
source ~/Android/env.sh
android/scripts/build-jni-guardian.sh release
```

Expected: `OK: guardian jniLibs built (release), 16 KB aligned.` and `.so` files under `android/carrier/src/main/jniLibs/{arm64-v8a,x86_64}/`.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add android/jni-guardian android/scripts/build-jni-guardian.sh
git commit -m "feat(carrier): guardian JNI surface + .so build script (16 KB gate)"
```

---

### Task 3: `:carrier` Gradle module — WebView shell with origin lock

**Files:**
- Modify: `android/settings.gradle.kts` (add `include(":carrier")`)
- Create: `android/carrier/build.gradle.kts`
- Create: `android/carrier/src/main/AndroidManifest.xml`
- Create: `android/carrier/src/main/res/values/strings.xml`, `android/carrier/src/main/res/values/themes.xml`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/MainActivity.kt`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/UrlGate.kt`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/native/GuardianNative.kt`
- Test: `android/carrier/src/test/kotlin/org/forgesworn/mycharter/web/UrlGateTest.kt`

**Interfaces:**
- Produces: `UrlGate.decide(url: String): UrlGate.Verdict` where `Verdict` ∈ `IN_APP` (load in WebView), `EXTERNAL` (hand to the system browser), `BLOCK` (drop). Console origin: `https://charter.mysignet.app` only.
- Produces: `GuardianNative.guardianAbiVersion()`, `GuardianNative.guardianClassifyWrap(...)` (Kotlin decls for Task 2's symbols).
- Produces: `MainActivity` with `EXTRA_ROUTE` (`"route"`) — `"approvals"` navigates the web app to `#/approvals` (used by Task 7's notification tap).

- [ ] **Step 1: Register the module**

In `android/settings.gradle.kts`, change the last line:

```kotlin
rootProject.name = "charter-android"
include(":app")
include(":carrier")
```

- [ ] **Step 2: Write the failing UrlGate test**

`android/carrier/src/test/kotlin/org/forgesworn/mycharter/web/UrlGateTest.kt`:

```kotlin
package org.forgesworn.mycharter.web

import org.junit.Assert.assertEquals
import org.junit.Test

class UrlGateTest {
    @Test fun consoleOriginStaysInApp() {
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/"))
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/#/approvals"))
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/pair/x"))
    }

    @Test fun foreignHttpsGoesExternal() {
        // The JS bridge must never be exposed to a foreign origin: anything
        // else opens in the system browser, not the WebView.
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://example.com/"))
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://charter.mysignet.app.evil.com/"))
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://mysignet.app/"))
    }

    @Test fun nonHttpSchemesAreBlocked() {
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("javascript:alert(1)"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("file:///etc/passwd"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("intent://x#Intent;end"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("not a url"))
    }

    @Test fun plainHttpNeverLoads() {
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("http://charter.mysignet.app/"))
    }
}
```

- [ ] **Step 3: Create the Gradle module so the test can run**

`android/carrier/build.gradle.kts`:

```kotlin
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.forgesworn.mycharter"
    compileSdk = 36

    defaultConfig {
        applicationId = "org.forgesworn.mycharter"
        // The guardian's own phone — not the DO-provisioned ward device — so
        // reach back further than the ward app's minSdk 34.
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Alpha: default debug key (same rationale as the ward app — see
            // android/app/build.gradle.kts; real signing is the sysadmin's, via env).
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    testImplementation("junit:junit:4.13.2")
    // Real org.json on the host-test classpath (android.jar stubs it out).
    testImplementation("org.json:json:20240303")
}
```

`android/carrier/src/main/res/values/strings.xml`:

```xml
<resources>
    <string name="app_name">MyCharter</string>
    <string name="notif_channel_approvals">Ward requests</string>
    <string name="notif_channel_service">Connection status</string>
</resources>
```

`android/carrier/src/main/res/values/themes.xml`:

```xml
<resources>
    <style name="Theme.MyCharter" parent="android:Theme.Material.Light.NoActionBar" />
</resources>
```

`android/carrier/src/main/AndroidManifest.xml` (Task 3 scope only — Task 7 adds the service/receiver):

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">

    <uses-permission android:name="android.permission.INTERNET" />

    <application
        android:label="@string/app_name"
        android:theme="@style/Theme.MyCharter"
        android:allowBackup="false">

        <activity
            android:name=".MainActivity"
            android:exported="true"
            android:launchMode="singleTask">
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
```

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/UrlGate.kt`:

```kotlin
package org.forgesworn.mycharter.web

import java.net.URI

/**
 * Navigation policy for the console WebView. The JS bridge (guardian key
 * hand-off) is exposed to every page the WebView loads, so ONLY the console
 * origin may load in-app; everything else goes to the system browser or is
 * dropped. Exact-host, https-only — no suffix matching.
 */
object UrlGate {
    const val CONSOLE_ORIGIN = "https://charter.mysignet.app"

    enum class Verdict { IN_APP, EXTERNAL, BLOCK }

    fun decide(url: String): Verdict {
        val uri = try { URI(url) } catch (_: Exception) { return Verdict.BLOCK }
        return when {
            uri.scheme == "https" && uri.host == "charter.mysignet.app" -> Verdict.IN_APP
            uri.scheme == "https" || uri.scheme == "http" ->
                if (uri.scheme == "http") Verdict.BLOCK else Verdict.EXTERNAL
            else -> Verdict.BLOCK
        }
    }
}
```

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/native/GuardianNative.kt`:

```kotlin
package org.forgesworn.mycharter.native

/**
 * Raw JNI declarations backed by libcharter_guardian_jni.so (android/jni-guardian,
 * cargo-ndk). Worker threads only — classify does real crypto.
 */
object GuardianNative {
    init {
        System.loadLibrary("charter_guardian_jni")
    }

    /** Surface version; Kotlin refuses to run on a mismatch. */
    external fun guardianAbiVersion(): Int

    /**
     * Classify one relay gift-wrap with the guardian secret. Returns JSON:
     * {"type":"request",…} | {"type":"status",…} | {"type":"other",…} |
     * {"type":"drop","reason":…}. Total — never throws for bad input.
     */
    external fun guardianClassifyWrap(
        wrapEventJson: String,
        guardianSkHex: String,
        nowUnix: Long,
    ): String
}
```

- [ ] **Step 4: Run the UrlGate test**

```bash
source ~/Android/env.sh
cd ~/charter/android
./gradlew :carrier:testDebugUnitTest --tests "org.forgesworn.mycharter.web.UrlGateTest" 2>&1 | tail -5
```

Expected: `BUILD SUCCESSFUL`, 4 tests passing.

- [ ] **Step 5: Add the WebView MainActivity**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/MainActivity.kt`:

```kotlin
package org.forgesworn.mycharter

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import org.forgesworn.mycharter.web.UrlGate

/**
 * The console shell: the MyCharter web app in a WebView. All decisions,
 * signing, and state live in the web app (its localStorage is the same
 * guardian identity the browser PWA would hold); this shell adds navigation
 * (notification tap → approvals) and, via CarrierBridge (Task 4), the key
 * hand-off that lets the native service classify wraps.
 */
class MainActivity : ComponentActivity() {

    private lateinit var webView: WebView

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        webView = WebView(this)
        setContentView(webView)

        webView.settings.javaScriptEnabled = true
        // The guardian key + app state live in localStorage — required.
        webView.settings.domStorageEnabled = true

        webView.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest,
            ): Boolean = when (UrlGate.decide(request.url.toString())) {
                UrlGate.Verdict.IN_APP -> false
                UrlGate.Verdict.EXTERNAL -> {
                    startActivity(Intent(Intent.ACTION_VIEW, request.url))
                    true
                }
                UrlGate.Verdict.BLOCK -> true
            }
        }

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (webView.canGoBack()) webView.goBack() else finish()
            }
        })

        webView.loadUrl(UrlGate.CONSOLE_ORIGIN + routeFragment(intent))
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val frag = routeFragment(intent)
        if (frag.isNotEmpty()) {
            // Same-document navigation: the hash router picks it up without a reload.
            webView.evaluateJavascript("window.location.hash='${frag.removePrefix("#")}'", null)
        }
    }

    private fun routeFragment(intent: Intent?): String =
        when (intent?.getStringExtra(EXTRA_ROUTE)) {
            ROUTE_APPROVALS -> "#/approvals"
            else -> ""
        }

    companion object {
        const val EXTRA_ROUTE = "route"
        const val ROUTE_APPROVALS = "approvals"
    }
}
```

- [ ] **Step 6: Build the APK**

```bash
./gradlew :carrier:assembleDebug 2>&1 | tail -3
```

Expected: `BUILD SUCCESSFUL`. (The `.so` from Task 2 rides along from `src/main/jniLibs`.)

- [ ] **Step 7: Commit**

```bash
cd ~/charter
git add android/settings.gradle.kts android/carrier
git commit -m "feat(carrier): :carrier module — MyCharter WebView shell with origin-locked navigation"
```

---

### Task 4: CarrierStore + JS bridge (`CharterCarrier`)

**Files:**
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/carrier/ProvisionPayload.kt`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/carrier/CarrierStore.kt`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/CarrierBridge.kt`
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/MainActivity.kt`
- Test: `android/carrier/src/test/kotlin/org/forgesworn/mycharter/carrier/ProvisionPayloadTest.kt`

**Interfaces:**
- Consumes: `UrlGate`, `MainActivity` (Task 3).
- Produces: `ProvisionPayload(guardianSkHex, guardianPubkeyHex, relays: List<String>)` with `ProvisionPayload.parse(json: String): ProvisionPayload?` (null on anything invalid).
- Produces: `CarrierStore(context)` — app-private SharedPreferences: `saveProvision(p)`, `provision(): ProvisionPayload?`, `markSeen(reqId): Boolean` (false if already seen; persists, capped LRU 200), `clear()`.
- Produces: JS-visible `window.CharterCarrier` with `@JavascriptInterface fun provision(json: String)` and `@JavascriptInterface fun isCarrier(): Boolean`.
- Produces: `CarrierService.start(context)` is invoked after a valid provision (Task 7 defines it; Task 4 calls a stub-safe `CarrierServiceStarter` lambda seam so this task stays independently testable).

- [ ] **Step 1: Write the failing ProvisionPayload tests**

`android/carrier/src/test/kotlin/org/forgesworn/mycharter/carrier/ProvisionPayloadTest.kt`:

```kotlin
package org.forgesworn.mycharter.carrier

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProvisionPayloadTest {
    private val sk = "a".repeat(64)
    private val pk = "b".repeat(64)

    private fun json(
        v: Int = 1,
        sk: String = this.sk,
        pk: String = this.pk,
        relays: String = """["wss://relay.trotters.cc"]""",
    ) = """{"v":$v,"guardianSkHex":"$sk","guardianPubkeyHex":"$pk","relays":$relays}"""

    @Test fun parsesAValidPayload() {
        val p = ProvisionPayload.parse(json())!!
        assertEquals(sk, p.guardianSkHex)
        assertEquals(pk, p.guardianPubkeyHex)
        assertEquals(listOf("wss://relay.trotters.cc"), p.relays)
    }

    @Test fun rejectsBadVersionBadHexAndEmptyRelays() {
        assertNull(ProvisionPayload.parse(json(v = 2)))
        assertNull(ProvisionPayload.parse(json(sk = "zz".repeat(32))))   // not hex
        assertNull(ProvisionPayload.parse(json(sk = "aa")))              // wrong len
        assertNull(ProvisionPayload.parse(json(pk = "B".repeat(64))))    // uppercase
        assertNull(ProvisionPayload.parse(json(relays = "[]")))
        assertNull(ProvisionPayload.parse(json(relays = """["http://x"]"""))) // not wss
        assertNull(ProvisionPayload.parse("not json"))
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd ~/charter/android
./gradlew :carrier:testDebugUnitTest --tests "org.forgesworn.mycharter.carrier.ProvisionPayloadTest" 2>&1 | tail -5
```

Expected: compile failure — `ProvisionPayload` unresolved.

- [ ] **Step 3: Implement ProvisionPayload**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/carrier/ProvisionPayload.kt`:

```kotlin
package org.forgesworn.mycharter.carrier

import org.json.JSONObject

/**
 * The key hand-off from the web console (CarrierBridge). Strict: v==1,
 * 64-char lowercase hex keys, ≥1 wss:// relay — anything else is null
 * (never a partial parse; a bad hand-off must not half-provision).
 */
data class ProvisionPayload(
    val guardianSkHex: String,
    val guardianPubkeyHex: String,
    val relays: List<String>,
) {
    companion object {
        private val HEX64 = Regex("^[0-9a-f]{64}$")

        fun parse(json: String): ProvisionPayload? {
            val o = try { JSONObject(json) } catch (_: Exception) { return null }
            if (o.optInt("v", 0) != 1) return null
            val sk = o.optString("guardianSkHex", "")
            val pk = o.optString("guardianPubkeyHex", "")
            if (!HEX64.matches(sk) || !HEX64.matches(pk)) return null
            val arr = o.optJSONArray("relays") ?: return null
            val relays = (0 until arr.length()).map { arr.optString(it, "") }
            if (relays.isEmpty() || relays.any { !it.startsWith("wss://") }) return null
            return ProvisionPayload(sk, pk, relays)
        }
    }
}
```

- [ ] **Step 4: Run to verify pass**

Same command as Step 2. Expected: 2 tests pass.

- [ ] **Step 5: Implement CarrierStore + the bridge, wire into MainActivity**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/carrier/CarrierStore.kt`:

```kotlin
package org.forgesworn.mycharter.carrier

import android.content.Context
import org.json.JSONArray

/**
 * App-private persistence for the carrier: the provisioned guardian key,
 * relays, and the seen-reqId LRU (dedupe across restarts — relays replay
 * stored wraps on every reconnect).
 *
 * Custody note (spec D2): the secret is stored app-private + FBE-at-rest,
 * the SAME posture as the WebView's localStorage copy one directory over.
 * Keystore-wrapping this copy would not raise the real bar while that copy
 * exists; proper custody arrives with the embedded-Signet vault component,
 * deliberately NOT half-built here.
 */
class CarrierStore(context: Context) {
    private val prefs = context.getSharedPreferences("carrier", Context.MODE_PRIVATE)

    fun saveProvision(p: ProvisionPayload) {
        prefs.edit()
            .putString(K_SK, p.guardianSkHex)
            .putString(K_PK, p.guardianPubkeyHex)
            .putString(K_RELAYS, JSONArray(p.relays).toString())
            .apply()
    }

    fun provision(): ProvisionPayload? {
        val sk = prefs.getString(K_SK, null) ?: return null
        val pk = prefs.getString(K_PK, null) ?: return null
        val relaysJson = prefs.getString(K_RELAYS, null) ?: return null
        val arr = try { JSONArray(relaysJson) } catch (_: Exception) { return null }
        val relays = (0 until arr.length()).map { arr.optString(it, "") }.filter { it.isNotEmpty() }
        if (relays.isEmpty()) return null
        return ProvisionPayload(sk, pk, relays)
    }

    /** True if this reqId is NEW (recorded now); false if already seen. */
    @Synchronized
    fun markSeen(reqId: String): Boolean {
        val arr = try { JSONArray(prefs.getString(K_SEEN, "[]")) } catch (_: Exception) { JSONArray() }
        val seen = (0 until arr.length()).map { arr.optString(it, "") }
        if (reqId in seen) return false
        val next = (seen + reqId).takeLast(SEEN_CAP)
        prefs.edit().putString(K_SEEN, JSONArray(next).toString()).apply()
        return true
    }

    fun clear() = prefs.edit().clear().apply()

    private companion object {
        const val K_SK = "guardianSkHex"
        const val K_PK = "guardianPubkeyHex"
        const val K_RELAYS = "relays"
        const val K_SEEN = "seenReqIds"
        const val SEEN_CAP = 200
    }
}
```

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/CarrierBridge.kt`:

```kotlin
package org.forgesworn.mycharter.web

import android.webkit.JavascriptInterface
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.ProvisionPayload

/**
 * `window.CharterCarrier` — what the console page sees. Exposure is safe
 * ONLY because UrlGate keeps every foreign origin out of this WebView.
 * The bridge is one-way and idempotent: the page pushes the provision
 * payload; the shell stores it and (re)starts the listening service.
 */
class CarrierBridge(
    private val store: CarrierStore,
    private val onProvisioned: () -> Unit,
) {
    @JavascriptInterface
    fun isCarrier(): Boolean = true

    @JavascriptInterface
    fun provision(json: String) {
        val p = ProvisionPayload.parse(json) ?: return
        store.saveProvision(p)
        onProvisioned()
    }

    companion object { const val JS_NAME = "CharterCarrier" }
}
```

In `MainActivity.onCreate`, after `webView.settings.domStorageEnabled = true` add:

```kotlin
        // Key hand-off from the console page. onProvisioned runs on the JS
        // bridge thread — CarrierService.start is safe from any thread.
        webView.addJavascriptInterface(
            CarrierBridge(CarrierStore(this)) { CarrierService.start(this) },
            CarrierBridge.JS_NAME,
        )
```

with imports `org.forgesworn.mycharter.web.CarrierBridge`, `org.forgesworn.mycharter.carrier.CarrierStore`, `org.forgesworn.mycharter.service.CarrierService`. Until Task 7 exists, add a temporary stub so the module compiles:

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/CarrierService.kt` (replaced wholesale in Task 7):

```kotlin
package org.forgesworn.mycharter.service

import android.content.Context

/** Task 7 replaces this stub with the real foreground service. */
object CarrierService {
    fun start(@Suppress("UNUSED_PARAMETER") context: Context) = Unit
}
```

- [ ] **Step 6: Full module build + tests**

```bash
./gradlew :carrier:assembleDebug :carrier:testDebugUnitTest 2>&1 | tail -3
```

Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 7: Commit**

```bash
cd ~/charter
git add android/carrier
git commit -m "feat(carrier): provision store + CharterCarrier JS bridge (strict payload parse)"
```

---

### Task 5: PWA carrier bridge — hand the key to the shell

**Files:**
- Create: `apps/charter-app/src/carrier/bridge.ts`
- Test: `apps/charter-app/src/carrier/bridge.test.ts`
- Modify: `apps/charter-app/src/App.tsx` (one call at key-ready)

**Interfaces:**
- Consumes: `readGuardianKey()` from `src/signer/guardianKey.ts` (`{kind:"ok",key}|{kind:"absent"}|{kind:"corrupt"}`), `DEFAULT_RELAYS` from `src/signer/config.ts`, `getPublicKey` from nostr-tools.
- Produces: `provisionCarrier(): boolean` — true iff running inside the carrier AND a key was handed off. Payload shape consumed by Task 4: `{"v":1,"guardianSkHex","guardianPubkeyHex","relays"}`.

- [ ] **Step 1: Write the failing vitest tests**

`apps/charter-app/src/carrier/bridge.test.ts`:

```typescript
import { afterEach, describe, expect, it, vi } from "vitest";
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { provisionCarrier } from "./bridge";

const KEY = "charter.guardian.key.v1";

declare global {
  interface Window {
    CharterCarrier?: { provision: (json: string) => void; isCarrier: () => boolean };
  }
}

afterEach(() => {
  localStorage.clear();
  delete window.CharterCarrier;
  vi.restoreAllMocks();
});

describe("provisionCarrier", () => {
  it("is a no-op outside the carrier", () => {
    localStorage.setItem(KEY, nip19.nsecEncode(generateSecretKey()));
    expect(provisionCarrier()).toBe(false);
  });

  it("hands the key + relays to the shell inside the carrier", () => {
    const sk = generateSecretKey();
    localStorage.setItem(KEY, nip19.nsecEncode(sk));
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };

    expect(provisionCarrier()).toBe(true);
    expect(provision).toHaveBeenCalledTimes(1);
    const payload = JSON.parse(provision.mock.calls[0][0]);
    expect(payload.v).toBe(1);
    expect(payload.guardianSkHex).toMatch(/^[0-9a-f]{64}$/);
    expect(payload.guardianPubkeyHex).toBe(getPublicKey(sk));
    expect(payload.relays.length).toBeGreaterThan(0);
    expect(payload.relays[0]).toMatch(/^wss:\/\//);
  });

  it("does not hand off when no key exists yet (absent) or the key is corrupt", () => {
    const provision = vi.fn();
    window.CharterCarrier = { provision, isCarrier: () => true };
    expect(provisionCarrier()).toBe(false); // absent — never mint from the bridge

    localStorage.setItem(KEY, "garbage-not-an-nsec");
    expect(provisionCarrier()).toBe(false); // corrupt — restore flow owns this
    expect(provision).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run to verify failure**

```bash
cd ~/charter/apps/charter-app
npx vitest run src/carrier/bridge.test.ts 2>&1 | tail -5
```

Expected: FAIL — module `./bridge` not found.

- [ ] **Step 3: Implement the bridge**

`apps/charter-app/src/carrier/bridge.ts`:

```typescript
// The MyCharter carrier APK injects `window.CharterCarrier` into this page
// (native Android shell, origin-locked to this console). When present, hand
// it the guardian key + relays so its foreground service can classify relay
// wraps and raise approval notifications while the app is closed.
//
// Read-only with respect to key state: NEVER mints (absent) and NEVER touches
// a corrupt value (the restore flow owns that) — mirrors readGuardianKey's
// no-silent-overwrite guarantee.

import { getPublicKey } from "nostr-tools";
import { bytesToHex } from "@noble/hashes/utils";
import { readGuardianKey } from "../signer/guardianKey";
import { DEFAULT_RELAYS } from "../signer/config";

export function provisionCarrier(): boolean {
  const carrier = window.CharterCarrier;
  if (!carrier) return false;
  const loaded = readGuardianKey();
  if (loaded.kind !== "ok") return false;
  carrier.provision(
    JSON.stringify({
      v: 1,
      guardianSkHex: bytesToHex(loaded.key),
      guardianPubkeyHex: getPublicKey(loaded.key),
      relays: DEFAULT_RELAYS,
    }),
  );
  return true;
}
```

(`@noble/hashes` is already a nostr-tools dependency; if the direct import upsets the build, use a local `const hex = Array.from(k).map(b => b.toString(16).padStart(2, "0")).join("")` instead — the test asserts the format either way.)

Type declaration — append to the test's `declare global` OR create `apps/charter-app/src/carrier/global.d.ts`:

```typescript
export {};
declare global {
  interface Window {
    CharterCarrier?: { provision: (json: string) => void; isCarrier: () => boolean };
  }
}
```

- [ ] **Step 4: Run to verify pass**

```bash
npx vitest run src/carrier/bridge.test.ts 2>&1 | tail -5
```

Expected: 3 tests pass.

- [ ] **Step 5: Call it at key-ready in App.tsx**

In `apps/charter-app/src/App.tsx`, the component already resolves the guardian-key state on mount (the corrupt-key gate around line 39–63). Add after the existing key-state effect (exact insertion point: right after the `useEffect` that installs the `hashchange` listener):

```tsx
  // Carrier hand-off: when running inside the MyCharter APK shell, give the
  // native service the key so asks can alert while the app is closed. Cheap
  // + idempotent, so re-running on every mount (incl. after restore) is fine.
  useEffect(() => {
    provisionCarrier();
  });
```

with `import { provisionCarrier } from "./carrier/bridge";`. Note: dependency-less `useEffect` (runs every render) is deliberate — after a restore-from-backup the key changes without a remount; the call is a cheap no-op otherwise.

- [ ] **Step 6: Full PWA gate**

```bash
cd ~/charter/apps/charter-app
npm test 2>&1 | tail -5 && npm run build 2>&1 | tail -3
```

Expected: all vitest suites pass; build succeeds.

- [ ] **Step 7: Commit**

```bash
cd ~/charter
git add apps/charter-app/src/carrier apps/charter-app/src/App.tsx
git commit -m "feat(charter-app): hand the guardian key to the MyCharter carrier shell when present"
```

---

### Task 6: Relay framing + subscription bookkeeping (pure Kotlin)

**Files:**
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/relay/RelayFraming.kt`
- Test: `android/carrier/src/test/kotlin/org/forgesworn/mycharter/relay/RelayFramingTest.kt`

**Interfaces:**
- Consumes: nothing (pure).
- Produces:
  - `RelayFraming.reqMessage(subId: String, guardianPubkeyHex: String, sinceUnix: Long): String` — `["REQ",subId,{"kinds":[1059],"#p":[pk],"since":n}]`
  - `RelayFraming.parseEvent(frame: String): String?` — the raw event JSON of an `["EVENT",…]` frame (null for EOSE/NOTICE/junk)
  - `RelayFraming.SINCE_WINDOW_SECS = 172_800L` — NIP-59 wraps carry randomized `created_at` up to ~2 days in the past, so every (re)subscribe reaches back 48 h; the seen-reqId LRU makes redelivery free.

- [ ] **Step 1: Write the failing tests**

`android/carrier/src/test/kotlin/org/forgesworn/mycharter/relay/RelayFramingTest.kt`:

```kotlin
package org.forgesworn.mycharter.relay

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RelayFramingTest {
    @Test fun reqMessageIsWellFormed() {
        val pk = "c".repeat(64)
        val msg = JSONArray(RelayFraming.reqMessage("carrier", pk, 1000L))
        assertEquals("REQ", msg.getString(0))
        assertEquals("carrier", msg.getString(1))
        val filter = msg.getJSONObject(2)
        assertEquals(1059, filter.getJSONArray("kinds").getInt(0))
        assertEquals(pk, filter.getJSONArray("#p").getString(0))
        assertEquals(1000L, filter.getLong("since"))
    }

    @Test fun parseEventExtractsTheEventJson() {
        val raw = """["EVENT","carrier",{"id":"ff","kind":1059,"content":"x"}]"""
        val ev = RelayFraming.parseEvent(raw)!!
        assertEquals(1059, org.json.JSONObject(ev).getInt("kind"))
    }

    @Test fun nonEventFramesAreNull() {
        assertNull(RelayFraming.parseEvent("""["EOSE","carrier"]"""))
        assertNull(RelayFraming.parseEvent("""["NOTICE","slow down"]"""))
        assertNull(RelayFraming.parseEvent("""["EVENT","carrier"]"""))
        assertNull(RelayFraming.parseEvent("not json"))
    }
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cd ~/charter/android
./gradlew :carrier:testDebugUnitTest --tests "org.forgesworn.mycharter.relay.RelayFramingTest" 2>&1 | tail -5
```

Expected: compile failure.

- [ ] **Step 3: Implement**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/relay/RelayFraming.kt`:

```kotlin
package org.forgesworn.mycharter.relay

import org.json.JSONArray
import org.json.JSONObject

/**
 * Minimal NIP-01 client framing for the carrier's one job: subscribe to
 * gift-wraps p-tagged to the guardian. Parsing is defensive — a relay frame
 * we don't understand is null, never a throw in the websocket callback.
 */
object RelayFraming {
    /**
     * NIP-59 randomizes wrap `created_at` up to ~2 days into the past, so a
     * tight `since` silently loses jittered wraps. Reach back 48 h on every
     * (re)subscribe; the persisted seen-reqId LRU makes redelivery free.
     */
    const val SINCE_WINDOW_SECS = 172_800L

    fun reqMessage(subId: String, guardianPubkeyHex: String, sinceUnix: Long): String =
        JSONArray()
            .put("REQ")
            .put(subId)
            .put(
                JSONObject()
                    .put("kinds", JSONArray().put(1059))
                    .put("#p", JSONArray().put(guardianPubkeyHex))
                    .put("since", sinceUnix),
            )
            .toString()

    fun parseEvent(frame: String): String? {
        val arr = try { JSONArray(frame) } catch (_: Exception) { return null }
        if (arr.length() < 3 || arr.optString(0) != "EVENT") return null
        return arr.optJSONObject(2)?.toString()
    }
}
```

- [ ] **Step 4: Run to verify pass**

Same command as Step 2. Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add android/carrier/src/main/kotlin/org/forgesworn/mycharter/relay android/carrier/src/test/kotlin/org/forgesworn/mycharter/relay
git commit -m "feat(carrier): NIP-01 relay framing with 48h jitter-safe since window"
```

---### Task 7: CarrierService — the always-on listener + notifications

**Files:**
- Replace: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/CarrierService.kt` (the Task 4 stub)
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/BootReceiver.kt`
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/Notifier.kt`
- Modify: `android/carrier/src/main/AndroidManifest.xml`
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/MainActivity.kt` (runtime permission + battery exemption ask)

**Interfaces:**
- Consumes: `CarrierStore` / `ProvisionPayload` (Task 4), `RelayFraming` (Task 6), `GuardianNative.guardianClassifyWrap` (Tasks 2–3), `MainActivity.EXTRA_ROUTE` / `ROUTE_APPROVALS` (Task 3).
- Produces: `CarrierService.start(context)` (idempotent; no-op unless provisioned), a foreground service holding one OkHttp websocket per relay, and `Notifier.notifyRequest(context, reqId, op, minutes)` posting the high-importance "ward asks" notification whose tap opens approvals.

- [ ] **Step 1: Manifest additions**

Add to `android/carrier/src/main/AndroidManifest.xml` — permissions block:

```xml
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_SPECIAL_USE" />
    <uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED" />
    <uses-permission android:name="android.permission.USE_FULL_SCREEN_INTENT" />
    <uses-permission android:name="android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS" />
```

and inside `<application>`:

```xml
        <service
            android:name=".service.CarrierService"
            android:foregroundServiceType="specialUse"
            android:exported="false">
            <property
                android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
                android:value="Realtime guardian approval alerts for supervised family devices" />
        </service>

        <receiver
            android:name=".service.BootReceiver"
            android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.BOOT_COMPLETED" />
                <action android:name="android.intent.action.MY_PACKAGE_REPLACED" />
            </intent-filter>
        </receiver>
```

- [ ] **Step 2: Implement Notifier**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/Notifier.kt`:

```kotlin
package org.forgesworn.mycharter.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import org.forgesworn.mycharter.MainActivity
import org.forgesworn.mycharter.R

/**
 * Notification surfaces. Two channels: the quiet persistent one the foreground
 * service is required to show, and the URGENT one a ward's ask rides in
 * (heads-up + sound + lockscreen). Copy uses the wardship lexicon.
 */
object Notifier {
    private const val CH_SERVICE = "carrier.service"
    private const val CH_APPROVALS = "carrier.approvals"
    const val SERVICE_NOTIF_ID = 1

    fun ensureChannels(ctx: Context) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(
                CH_SERVICE,
                ctx.getString(R.string.notif_channel_service),
                NotificationManager.IMPORTANCE_MIN,
            ),
        )
        nm.createNotificationChannel(
            NotificationChannel(
                CH_APPROVALS,
                ctx.getString(R.string.notif_channel_approvals),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                enableVibration(true)
            },
        )
    }

    fun serviceNotification(ctx: Context): Notification =
        Notification.Builder(ctx, CH_SERVICE)
            .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
            .setContentTitle("MyCharter is listening for ward requests")
            .setOngoing(true)
            .build()

    /** The ask itself. Tap → the console's Approvals screen. */
    fun notifyRequest(ctx: Context, reqId: String, op: String, minutes: Long?) {
        val tap = PendingIntent.getActivity(
            ctx,
            reqId.hashCode(),
            Intent(ctx, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_ROUTE, MainActivity.ROUTE_APPROVALS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = when {
            op == "time.extend" && minutes != null -> "Your ward asks for $minutes more minutes"
            op == "install.apk" -> "Your ward asks to install an app"
            else -> "Your ward sent a request"
        }
        val n = Notification.Builder(ctx, CH_APPROVALS)
            .setSmallIcon(android.R.drawable.stat_notify_more)
            .setContentTitle("Charter request")
            .setContentText(text)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            // Full-screen intent: lights the screen where permitted (Android
            // 14+ may quietly downgrade to heads-up — acceptable fallback).
            .setFullScreenIntent(tap, true)
            .build()
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        // One notification per reqId — a replayed wrap never re-alerts anyway
        // (store dedupe), but distinct asks each get their own row.
        nm.notify(reqId.hashCode(), n)
    }
}
```

- [ ] **Step 3: Implement the service + boot receiver**

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/CarrierService.kt` (replaces the stub):

```kotlin
package org.forgesworn.mycharter.service

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.native.GuardianNative
import org.forgesworn.mycharter.relay.RelayFraming
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * The carrier's whole point: a foreground service holding one websocket per
 * relay, classifying every gift-wrap addressed to the guardian, and raising
 * an URGENT notification for each unseen ward REQUEST. No FCM — the socket
 * IS the push channel, which keeps de-Googled guardian phones first-class.
 *
 * Reliability posture: START_STICKY + BootReceiver + exponential-backoff
 * reconnect (1s→64s cap) + OkHttp pings. Classification runs on a worker
 * thread (JNI rule). Everything is level-triggered off the persisted
 * provision — a restart resubscribes from (now - 48h) and the seen-reqId LRU
 * absorbs the replay.
 */
class CarrierService : Service() {

    private lateinit var store: CarrierStore
    private lateinit var worker: HandlerThread
    private lateinit var handler: Handler
    private val sockets = mutableMapOf<String, WebSocket>()
    private val backoffs = mutableMapOf<String, Long>()
    @Volatile private var running = false

    private val client = OkHttpClient.Builder()
        .pingInterval(30, TimeUnit.SECONDS)
        .build()

    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
        startForeground(Notifier.SERVICE_NOTIF_ID, Notifier.serviceNotification(this))
        store = CarrierStore(this)
        worker = HandlerThread("carrier-worker").apply { start() }
        handler = Handler(worker.looper)
        running = true
        handler.post { connectAll() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        running = false
        sockets.values.forEach { it.close(1000, "service stopped") }
        sockets.clear()
        worker.quitSafely()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    // ---- relay plumbing (worker thread) ----------------------------------

    private fun connectAll() {
        val p = store.provision() ?: run {
            Log.i(TAG, "not provisioned; stopping")
            stopSelf()
            return
        }
        val abi = GuardianNative.guardianAbiVersion()
        check(abi == 1) { "guardian JNI ABI $abi != 1" }
        p.relays.forEach { relay -> if (relay !in sockets) connect(relay) }
    }

    private fun connect(relay: String) {
        if (!running) return
        val p = store.provision() ?: return
        val since = System.currentTimeMillis() / 1000 - RelayFraming.SINCE_WINDOW_SECS
        val ws = client.newWebSocket(
            Request.Builder().url(relay).build(),
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    Log.i(TAG, "open $relay")
                    backoffs[relay] = 0
                    webSocket.send(RelayFraming.reqMessage("carrier", p.guardianPubkeyHex, since))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    val eventJson = RelayFraming.parseEvent(text) ?: return
                    handler.post { classifyAndNotify(eventJson) }
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    Log.w(TAG, "socket failed $relay: ${t.message}")
                    scheduleReconnect(relay)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    Log.i(TAG, "closed $relay ($code)")
                    scheduleReconnect(relay)
                }
            },
        )
        sockets[relay] = ws
    }

    private fun scheduleReconnect(relay: String) {
        if (!running) return
        sockets.remove(relay)
        val next = ((backoffs[relay] ?: 0) * 2).coerceIn(1, 64)
        backoffs[relay] = next
        handler.postDelayed({ connect(relay) }, next * 1000)
    }

    private fun classifyAndNotify(eventJson: String) {
        val p = store.provision() ?: return
        val now = System.currentTimeMillis() / 1000
        val verdict = try {
            JSONObject(GuardianNative.guardianClassifyWrap(eventJson, p.guardianSkHex, now))
        } catch (t: Throwable) {
            Log.w(TAG, "classify failed: ${t.message}")
            return
        }
        if (verdict.optString("type") != "request") return
        val reqId = verdict.optString("reqId", "")
        if (reqId.isEmpty() || !store.markSeen(reqId)) return
        val params = verdict.optJSONObject("params")
        val minutes = params?.optLong("minutesRequested", -1L).takeIf { it != null && it >= 0 }
        Log.i(TAG, "ward request $reqId (${verdict.optString("op")})")
        Notifier.notifyRequest(this, reqId, verdict.optString("op"), minutes)
    }

    companion object {
        private const val TAG = "CarrierService"

        /** Idempotent start; safe from any thread; no-op unless provisioned. */
        fun start(context: Context) {
            if (CarrierStore(context).provision() == null) return
            context.startForegroundService(Intent(context, CarrierService::class.java))
        }
    }
}
```

`android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/BootReceiver.kt`:

```kotlin
package org.forgesworn.mycharter.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Re-arm the listener after boot or app update — provisioned carriers only. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED ->
                CarrierService.start(context)
        }
    }
}
```

- [ ] **Step 4: MainActivity — permission + battery exemption + service kick**

Add to `MainActivity.onCreate` (end of the method), with imports `android.Manifest`, `android.content.pm.PackageManager`, `android.net.Uri`, `android.os.Build`, `android.os.PowerManager`, `android.provider.Settings`, `org.forgesworn.mycharter.service.CarrierService`:

```kotlin
        // Notifications are the product — ask up front (Android 13+).
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        // A dozed carrier is a deaf carrier: ask once for the doze exemption.
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        if (!pm.isIgnoringBatteryOptimizations(packageName)) {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                    .setData(Uri.parse("package:$packageName")),
            )
        }
        // Re-arm on every open (idempotent; no-op until provisioned).
        CarrierService.start(this)
```

- [ ] **Step 5: Build + all module tests**

```bash
source ~/Android/env.sh
cd ~/charter/android
./gradlew :carrier:assembleDebug :carrier:testDebugUnitTest 2>&1 | tail -3
```

Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 6: Commit**

```bash
cd ~/charter
git add android/carrier
git commit -m "feat(carrier): always-on foreground listener — websocket sub, classify, urgent ward-request notifications"
```

---

### Task 8: `fire_ask` example — the autonomous end-to-end round

**Files:**
- Create: `android/jni/examples/fire_ask.rs`
- Modify: `android/jni/Cargo.toml` (register the example)

**Interfaces:**
- Consumes: the same crates/patterns as `live_guardian.rs` (`RealRelayTransport`, `nip59::wrap`, `test_support::sign_event`).
- Produces: `cargo run --example fire_ask --features mock -- <guardian_pk_hex> [minutes]` — publishes ONE real kind-31111 REQUEST wrap (from a throwaway machine key) addressed to the guardian over `wss://relay.trotters.cc`. This is the test gun for the emulator round and every future hardware round.

- [ ] **Step 1: Register the example**

Append to `android/jni/Cargo.toml`:

```toml
[[example]]
name = "fire_ask"
required-features = ["mock"]
```

- [ ] **Step 2: Implement**

`android/jni/examples/fire_ask.rs`:

```rust
//! Fire ONE test ask at a guardian: publish a kind-31111 time.extend REQUEST
//! (from a throwaway machine key) gift-wrapped to <guardian_pk_hex> over the
//! real relay. The carrier APK's end-to-end round: run this, watch the
//! guardian phone light up.
//!
//!   cargo run --example fire_ask --features mock -- <guardian_pk_hex> [minutes]

use std::time::{SystemTime, UNIX_EPOCH};

use charter_primitives::{kinds, Nonce, PubKey, ReqId};
use charter_proto::{OpType, RequestPayload};
use charter_sys::relay::{RealRelayTransport, RelayTransport};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_verify::test_support::sign_event;

const RELAY: &str = "wss://relay.trotters.cc";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs()
}

fn rand32() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("entropy");
    b
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let guardian = PubKey::from_hex(
        &std::env::args().nth(1).expect("usage: fire_ask <guardian_pk_hex> [minutes]"),
    )
    .expect("valid guardian pubkey hex");
    let minutes: u64 = std::env::args().nth(2).and_then(|m| m.parse().ok()).unwrap_or(10);

    // Throwaway machine identity — a fresh one per shot is fine: the carrier
    // notifies on any authenticated ask addressed to the guardian.
    let machine_sk = rand32();
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).expect("sk"));

    let ts = now();
    let payload = RequestPayload {
        v: 1,
        op: OpType::TimeExtend,
        req_id: ReqId::from_bytes(rand32()),
        nonce: Nonce::from_bytes(rand32()),
        subject: machine_pk,
        machine: machine_pk,
        ts,
        params: serde_json::json!({"minutesRequested": minutes, "limitHit": "budget"}),
    };
    let signer = charter_sys::signer::SeedSigner::from_secret(machine_sk);
    let ev = sign_event(
        &signer,
        kinds::CHARTER_DEVICE_REQUEST,
        ts,
        vec![kinds::marker_tag()],
        payload.to_json(),
    );
    let wrap = nip59::wrap(
        &Rumor::from_signed_event(&ev),
        &machine_sk,
        guardian.as_bytes(),
        &WrapRandomness {
            ephemeral_secret: rand32(),
            seal_nonce: rand32(),
            wrap_nonce: rand32(),
            seal_created_at: ts,
            wrap_created_at: ts,
        },
    )
    .expect("wrap");

    let relay = RealRelayTransport::default();
    let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
    println!(
        "fired ask reqId={} ({minutes} min) at guardian {}: {outcomes:?}",
        payload.req_id.to_hex(),
        guardian.to_hex(),
    );
}
```

- [ ] **Step 3: Verify it compiles and fires**

```bash
cd ~/charter/android/jni
cargo run --example fire_ask --features mock -- $(printf 'ab%.0s' {1..32}) 5 2>&1 | tail -3
```

Expected: `fired ask reqId=… (5 min) at guardian abab…: [Ok…]` (the throwaway guardian pk means nobody is listening — this step only proves publish works).

- [ ] **Step 4: Commit**

```bash
cd ~/charter
git add android/jni/examples/fire_ask.rs android/jni/Cargo.toml
git commit -m "feat(carrier): fire_ask example — one-shot test REQUEST for carrier e2e rounds"
```

---

### Task 9: Emulator end-to-end round + release artifact

**Files:**
- Create: `docs/superpowers/specs/2026-07-22-carrier-emulator-round.md` (the evidence log)

This is a verification task — the full loop on the `charter-ci` AVD, no phone needed.

- [ ] **Step 1: Boot the emulator and install the carrier**

```bash
source ~/Android/env.sh
emulator -avd charter-ci -gpu off -no-window -no-audio &
adb wait-for-device
cd ~/charter/android
./gradlew :carrier:installDebug
adb shell am start -n org.forgesworn.mycharter/.MainActivity
```

Expected: app launches (verify with `adb shell dumpsys activity activities | grep mycharter`). The WebView loads the LIVE console (charter.mysignet.app) — Task 5's bridge must already be deployed for the hand-off; if the site hasn't shipped yet, provision manually for this round:

```bash
# Manual provision fallback (debug builds only): drive the JS bridge from a
# throwaway key pair. Generate one with the jni example tooling:
cd android/jni && cargo run --example live_guardian --features mock -- uri
# → prints "guardian pubkey: <PK>"; the sk hex is in target/live-guardian.key
SK=$(cat target/live-guardian.key)
PK=$(cargo run --example live_guardian --features mock -- uri 2>/dev/null | awk '/guardian pubkey/{print $3}')
adb shell "run-as org.forgesworn.mycharter sh -c 'echo ok'" || true
# Simplest reliable injection: evaluate JS in the WebView via chrome devtools
# is heavyweight — instead use the app UI: open the app, and in the console
# WebView complete the normal MyCharter first-run (it mints a key and the
# bridge hands it off automatically once Task 5 is deployed).
```

(If the deployed site predates Task 5: push `apps/charter-app` to main first — the deploy is automatic — then relaunch the app. Note the ordering in the evidence log.)

- [ ] **Step 2: Capture the provisioned guardian pubkey**

```bash
adb shell "run-as org.forgesworn.mycharter cat shared_prefs/carrier.xml" | grep guardianPubkeyHex
```

Expected: the 64-hex pubkey. Export it: `GPK=<value>`.

- [ ] **Step 3: Close the app (the whole point) and fire an ask**

```bash
adb shell input keyevent KEYCODE_HOME   # background it — do NOT force-stop (that would kill the service)
cd ~/charter/android/jni
cargo run --example fire_ask --features mock -- "$GPK" 10
```

(NB: `force-stop` would kill the service — backgrounding via HOME is the honest test. A swipe-from-recents test is a later hardware-round item.)

- [ ] **Step 4: Verify the notification arrived**

```bash
sleep 5
adb shell dumpsys notification --noredact | grep -B2 -A8 "mycharter"
adb exec-out screencap -p > /tmp/carrier-notif.png 2>/dev/null || adb exec-out screencap -p > carrier-notif.png
```

Expected: a posted notification from `org.forgesworn.mycharter`, channel `carrier.approvals`, text containing "10 more minutes". Save the dumpsys excerpt + screenshot into the evidence log.

- [ ] **Step 5: Verify the tap route and the dedupe**

```bash
# Tap-equivalent: fire the content intent directly, confirm approvals route.
adb shell am start -n org.forgesworn.mycharter/.MainActivity -e route approvals
# Re-fire the SAME reqId scenario: publish again with fire_ask (new reqId → new
# notification is CORRECT); then restart the service and confirm the OLD reqId
# does not re-alert:
adb shell am force-stop org.forgesworn.mycharter
adb shell am start -n org.forgesworn.mycharter/.MainActivity
sleep 8
adb shell dumpsys notification --noredact | grep -c "carrier.approvals"
```

Expected: after restart + resubscribe (48 h window replays the wrap) the seen-LRU suppresses a duplicate alert — the count does not grow.

- [ ] **Step 6: Release artifact**

```bash
cd ~/charter
android/scripts/build-jni-guardian.sh release
cd android && ./gradlew :carrier:assembleRelease
ls -la carrier/build/outputs/apk/release/
```

Expected: `carrier-release.apk` (debug-key signed during alpha — same continuity story as the ward app).

- [ ] **Step 7: Write the evidence log + commit**

`docs/superpowers/specs/2026-07-22-carrier-emulator-round.md`: date, emulator image, the dumpsys excerpt proving the notification, the dedupe count, the APK sha256, and the explicit list of what is NOT yet proven (doze survival, swipe-away restart, real-phone lockscreen behaviour, battery cost — all hardware-round items for decented).

```bash
cd ~/charter
git add docs/superpowers/specs/2026-07-22-carrier-emulator-round.md
git commit -m "test(carrier): emulator end-to-end round — fire_ask → urgent notification, dedupe across restart"
```

---

## Explicitly Deferred (do NOT build in this plan)

- **ApprovalAlertActivity over the lockscreen** (full-screen wake surface with `setShowWhenLocked`/`requestDismissKeyguard`): v1 ships `setFullScreenIntent` pointing at MainActivity — Android decides heads-up vs full-screen. The dedicated keyguard-safe alert activity is the first hardware-round follow-up, designed against real lockscreen behaviour.
- **WebUSB ward provisioning inside the carrier** — WebView has no WebUSB; ward-phone provisioning stays in a real browser. Known, documented, fine.
- **Publishing the carrier APK on the website** (`publish-apk.sh` sibling) — after decented's first hardware round proves the artifact.
- **Keystore-wrapping the key copy / any custody feature** — D2: that's the embedded-Signet vault's job, later.
- **Web Push in the PWA** (desktop-guardian secondary nudge) — separate, later.

## Self-Review Notes

- Spec coverage: D5 (carrier, no-FCM, wake channel) fully; D1/D3 untouched (web app keeps adjudication + signing); D2 honoured by NOT building custody. D8/D9 are ward-side — out of scope here by design.
- Type consistency: `classify_wrap` JSON keys (`type/reqId/op/machine/params`) match `CarrierService.classifyAndNotify` reads; `ProvisionPayload` JSON keys match `bridge.ts` payload; `EXTRA_ROUTE`/`ROUTE_APPROVALS` match Notifier's intent and Step 5 of Task 9.
- Known API risks called out inline: `TestGuardian` secret accessor (Task 1 note), `OpType` serde shape (Task 1/4 note), `@noble/hashes` import (Task 5 note). Each has a stated fallback; the tests are the contract.
