# Charter Self-Update Implementation Plan (#44)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Guardian taps "Update Charter" in MyCharter → the chartered phone downloads, verifies, and silently installs the new APK as Device Owner, and MyCharter shows the new version — no adb.

**Architecture:** Self-update is a **clause** (`ClauseKind::Update`, store_key 8) — standing guardian-signed policy "this device runs ≥ versionCode X from URL U", absorbed like tethering and converged level-triggered — NOT an `install.apk` grant (grants are request-bound: they must echo a device-originated request's reqId+nonce, and no request exists for a guardian-initiated update). The warden compares the clause's versionCode to its own (new: reported in STATUS), queues a `PendingInstall` with `source:"url"`, Kotlin downloads + SHA-256-pins + hands to the existing cert-pinned DO installer. The existing installed-version idempotence clears the directive after the self-update restart.

**Tech Stack:** Rust (charter-proto, android/jni warden), Kotlin (UrlStager), TypeScript/React (MyCharter PWA), bash (publish script).

## Global Constraints

- Wire changes are additive + backward-compatible; `STATUS_VERSION` stays 1; new STATUS field is `Option` + `skip_serializing_if`.
- Clause body validation is fail-closed: https-only URL, 64-hex sha256, `valid_package_name`.
- Two independent pins must both hold before commit: APK sha256 (from the clause, checked by the stager) and signing-cert sha256 (from the clause, checked by `DpmApkInstallOps` pre-commit — existing code).
- Kotlin/JNI are NOT in CI — run `cargo fmt/clippy/test` in `android/jni`, `npm run typecheck && npm test` in `apps/charter-app`, and `./gradlew assembleDebug` locally; the four repo gates run from `linux/`.
- Wardship lexicon in user-facing copy (guardian/ward/charter/clause).
- Commits end with the Claude-Session trailer.

---

### Task 1: `ClauseKind::Update` + `UpdateAppBody` (charter-proto)

**Files:**
- Modify: `core/crates/charter-proto/src/clause.rs` (enum + store_key + new body struct + tests)
- Modify: `docs/superpowers/specs/2026-07-21-charter-self-update-design.md` (record the grant→clause correction)

**Interfaces:**
- Produces: `ClauseKind::Update` (serde `"update"`, `store_key() == 8`); `pub struct UpdateAppBody { v: u32, package_name: String, version_code: u64, version_name: String, url: String, apk_sha256: Sha256Hex, signer_cert_sha256: Sha256Hex }` (camelCase serde) with `pub fn validate(&self) -> Result<(), ProtoError>`.

- [ ] **Step 1: Failing tests** — in `clause.rs` `mod tests`, mirroring `tethering_kind_serializes_lowercase_with_distinct_store_key`:

```rust
#[test]
fn update_kind_serializes_lowercase_with_distinct_store_key() {
    let k = ClauseKind::Update;
    assert_eq!(serde_json::to_string(&k).unwrap(), "\"update\"");
    let back: ClauseKind = serde_json::from_str("\"update\"").unwrap();
    assert_eq!(back, ClauseKind::Update);
    assert_eq!(k.store_key(), 8);
}

#[test]
fn update_body_validates_fail_closed() {
    let good = UpdateAppBody {
        v: 1,
        package_name: "org.forgesworn.charter".into(),
        version_code: 21,
        version_name: "0.21.0".into(),
        url: "https://charter.mysignet.app/charter-latest.apk".into(),
        apk_sha256: Sha256Hex::parse(&"a".repeat(64)).unwrap(),
        signer_cert_sha256: Sha256Hex::parse(&"b".repeat(64)).unwrap(),
    };
    assert!(good.validate().is_ok());
    // http (not https) → rejected
    let mut b = good.clone();
    b.url = "http://charter.mysignet.app/charter-latest.apk".into();
    assert!(b.validate().is_err());
    // bad package name → rejected
    let mut b = good.clone();
    b.package_name = "not-a-package".into();
    assert!(b.validate().is_err());
    // versionCode 0 → rejected
    let mut b = good;
    b.version_code = 0;
    assert!(b.validate().is_err());
}
```

- [ ] **Step 2: Run to verify failure** — `cd core && cargo test -p charter-proto update_` → FAIL (no `Update` variant / no `UpdateAppBody`).

- [ ] **Step 3: Implement** — add `Update` to `ClauseKind` (doc comment: guardian-directed Charter self-update; replace-the-state like `tethering`), `ClauseKind::Update => 8` in `store_key()`, and the body struct (reuse `Sha256Hex`, `valid_package_name` from `params.rs`):

```rust
/// The `update` clause body: "this device runs `package_name` at
/// `version_code` or newer, fetched from `url`". Level-triggered — a device
/// already at or past the version does nothing. Both digests must hold on
/// the device before commit (archive sha256 in the stager, signing cert in
/// the installer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAppBody {
    pub v: u32,
    pub package_name: String,
    pub version_code: u64,
    pub version_name: String,
    pub url: String,
    pub apk_sha256: Sha256Hex,
    pub signer_cert_sha256: Sha256Hex,
}

impl UpdateAppBody {
    pub fn validate(&self) -> Result<(), ProtoError> {
        if !crate::params::valid_package_name(&self.package_name) {
            return Err(ProtoError::BadParams("invalid packageName".into()));
        }
        if self.version_code == 0 {
            return Err(ProtoError::BadParams("versionCode must be positive".into()));
        }
        if !self.url.starts_with("https://") {
            return Err(ProtoError::BadParams("update url must be https".into()));
        }
        Ok(())
    }
}
```

(If `Sha256Hex::parse` is named differently, use the existing constructor — check `params.rs` usage; the type already guarantees 64-hex.)

- [ ] **Step 4: Run to verify pass** — `cargo test -p charter-proto` → all green.
- [ ] **Step 5: Spec correction** — in the spec's §2, replace the `ApkSource::Url`-on-grants paragraph with the clause design (one short paragraph, reason: grants are request-bound).
- [ ] **Step 6: Commit** — `feat(proto): update clause — guardian-directed self-update wire (#44)`.

---

### Task 2: STATUS reports `appVersionCode`

**Files:**
- Modify: `core/crates/charter-proto/src/status.rs` (field + test)
- Modify: `core/crates/charter-spine/src/status_emit.rs` (`build_status` signature + passthrough)
- Modify: `android/jni/src/warden.rs` (store the version; pass to build_status)
- Modify: `android/jni/src/lib.rs` (`charterInit` gains `appVersionCode: jlong`)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterNative.kt` + `CharterCore.kt` (`init(baseDir, enforceMode, appVersionCode)`)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt` (pass `BuildConfig.VERSION_CODE.toLong()`)

**Interfaces:**
- Produces: `StatusPayload.app_version_code: Option<u64>` (serde `appVersionCode`, skip-if-none); `Warden::init(base, mode, app_version_code: u64)`; Kotlin `CharterCore.init(baseDir, enforceMode, appVersionCode: Long)`.

- [ ] **Step 1: Failing test** — in `status.rs` tests: a `StatusPayload` with `app_version_code: Some(21)` round-trips through JSON with key `appVersionCode`; with `None` the key is absent from the serialized string.
- [ ] **Step 2: Run** — `cargo test -p charter-proto status` → FAIL (no field).
- [ ] **Step 3: Implement** — add to `StatusPayload`:

```rust
/// The device's own Charter app versionCode — lets the guardian see
/// update state. Absent on older devices.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub app_version_code: Option<u64>,
```

Thread through `build_status(...)` (new `app_version_code: Option<u64>` param), fix its unit tests and all call sites (`android/jni/src/warden.rs`). In the JNI warden add `app_version_code: u64` field set from the new init param; pass `Some(self.app_version_code)` when emitting. Update `charterInit` JNI signature (`jlong app_version_code`), `CharterNative.charterInit`, `CharterCore.init`, and `WardenController.init` → `CharterCore.init(Provisioning.baseDir(context), enforceMode, org.forgesworn.charter.BuildConfig.VERSION_CODE.toLong())`.

- [ ] **Step 4: Run** — `cargo test -p charter-proto -p charter-spine` green; `cd android/jni && cargo test` green (fix `Warden::init` test callers with a version like 20).
- [ ] **Step 5: Commit** — `feat(status): devices report their app versionCode (#44)`.

---

### Task 3: JNI — url fields on the install queue + the self-update check

**Files:**
- Modify: `android/jni/src/install.rs` (PendingInstall fields; keep staged enact unchanged)
- Modify: `android/jni/src/warden.rs` (`check_self_update`, called from the poll path after clause absorb; tests)
- Modify: `android/jni/src/dto.rs` (PendingInstall dto → JSON out to Kotlin: `url`, `apkSha256`)

**Interfaces:**
- Consumes: `ClauseKind::Update` + `UpdateAppBody` (Task 1); `self.app_version_code` (Task 2).
- Produces: `PendingInstall { url: Option<String>, apk_sha256: Option<String>, .. }` (serde camelCase); queue items with `req_id = format!("self-update-{version_code}")`, `source = "url"`.

- [ ] **Step 1: Failing test** — in `warden.rs` tests (mirror `tethering_clause_drives_mode_and_expires`, which stores a clause directly):

```rust
/// The update clause (store_key 8) queues a url install when the device is
/// behind, exactly once, and does nothing at-or-past the version.
#[test]
fn update_clause_queues_url_install_when_behind() {
    // Warden inited with app_version_code = 20 (Task 2 init param).
    // Store an Update clause: versionCode 21, url https://…, both digests.
    // After check (driven via the poll/tick path used by the tethering test):
    //   - drain_installs(now) yields exactly one PendingInstall:
    //     req_id == "self-update-21", source == "url", url/apk_sha256 == clause's.
    // Second check: queue stays empty (req_id dedupe).
    // Warden inited with app_version_code = 21: same clause queues NOTHING.
}
```

(Write it concretely against the same helpers the tethering test uses — clause stored via the same store API, `drain_installs` to observe the queue.)

- [ ] **Step 2: Run** — `cargo test update_clause` → FAIL.
- [ ] **Step 3: Implement** —
  - `PendingInstall` gains `#[serde(default)] pub url: Option<String>` and `#[serde(default)] pub apk_sha256: Option<String>`; dto passthrough to the Kotlin-facing JSON.
  - `Warden::check_self_update(&mut self, now: u64)`: read clause store_key 8 → parse `UpdateAppBody` → `validate()` (fail-closed: invalid body = ignore + log) → require `body.package_name == own package` ("org.forgesworn.charter" constant) → if `body.version_code > self.app_version_code` and queue has no `self-update-{version_code}` entry → `queue.put(PendingInstall { req_id, package_name, version_code: Some(body.version_code), signer_cert_sha256: body.signer_cert_sha256.to_hex(), source: "url".into(), url: Some(body.url), apk_sha256: Some(body.apk_sha256.to_hex()), at: now })`.
  - Call `check_self_update` in the poll path right after clause absorb (same lock phase that runs the broker mirror), so a freshly absorbed clause acts within one poll.
- [ ] **Step 4: Run** — `cargo test` (android/jni) all green.
- [ ] **Step 5: Commit** — `feat(jni): update clause queues a url self-install when the device is behind (#44)`.

---

### Task 4: Kotlin — UrlStager + url branch in the installer

**Files:**
- Create: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/UrlStager.kt`
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/ApkInstallOps.kt` (accept `source == "url"`: stage first, then existing staged path)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterCore.kt` (`PendingInstall` gains `url: String?`, `apkSha256: String?` parsed from the JSON)
- Test: `android/app/src/test/kotlin/org/forgesworn/charter/enforce/UrlStagerTest.kt`

**Interfaces:**
- Consumes: `PendingInstall.url/apkSha256` (Task 3).
- Produces: `class UrlStager { fun stage(url: String, expectedSha256: String, dest: File): StageResult }` where `enum class StageResult { OK, HASH_MISMATCH, NETWORK }`.

- [ ] **Step 1: Failing tests** (JVM, local `HttpURLConnection` against a `ServerSocket`/`com.sun.net.httpserver.HttpServer` fixture, matching the existing hotspot test style):

```kotlin
class UrlStagerTest {
    // serves bytes; stage() with the matching sha256 → OK, dest exists, bytes equal
    @Test fun stagesAndVerifiesMatchingHash() { … }
    // serves bytes; stage() with a wrong sha256 → HASH_MISMATCH and dest deleted
    @Test fun deletesOnHashMismatch() { … }
    // connection refused → NETWORK, dest absent
    @Test fun networkFailureIsTransient() { … }
}
```

- [ ] **Step 2: Run** — `./gradlew testDebugUnitTest --tests '*UrlStagerTest*'` → FAIL.
- [ ] **Step 3: Implement** — `UrlStager`: stream download to `dest.tmp` while updating a `MessageDigest("SHA-256")`; on EOF compare lowercase hex to `expectedSha256` → rename to `dest` (OK) or delete (HASH_MISMATCH); IO/HTTP≠200 → delete + NETWORK. In `DpmApkInstallOps.install`: when `item.source == "url"` require `item.url != null && item.apkSha256 != null` (else TERMINAL, log), run the stager to `stagingDir/<packageName>.apk` (`OK` → fall through to the existing staged verification+commit; `HASH_MISMATCH` → TERMINAL; `NETWORK` → TRANSIENT). No change to the cert-continuity gate — it runs on the staged bytes as before.
- [ ] **Step 4: Run** — unit tests green; `./gradlew assembleDebug` compiles.
- [ ] **Step 5: Commit** — `feat(android): url-sourced installs — download, pin sha256, hand to the DO installer (#44)`.

---

### Task 5: MyCharter — version display, Update button, clause issuance

**Files:**
- Modify: `apps/charter-app/src/wire/status.ts` (`appVersionCode` optional parse)
- Modify: `apps/charter-app/src/wire/clause.ts` (`updateToGrant(u: UpdateDirective, issuedAt): ClausePayload` builder, kind `"update"`)
- Modify: `apps/charter-app/src/wire/types.ts` (types)
- Create: `apps/charter-app/src/wire/updateClauseVectors.test.ts` (mirror `tetheringClauseVectors.test.ts`: byte-stable golden vector)
- Modify: `apps/charter-app/src/screens/Family.tsx` (device card: version line + Update button + sent/updated states)
- Create: `apps/charter-app/src/store/updateCheck.ts` + `updateCheck.test.ts` (fetch `/charter-apk.json`, compare, expose `{available, manifest}`)

**Interfaces:**
- Consumes: STATUS `appVersionCode` (Task 2); site manifest `{versionName, versionCode, apkSha256, certSha256, sizeBytes, builtAt}` (Task 6 emits it; this task freezes the shape).
- Produces: the `update` clause body exactly matching Task 1's `UpdateAppBody` serde (camelCase: `v, packageName, versionCode, versionName, url, apkSha256, signerCertSha256`).

- [ ] **Step 1: Failing tests** — vitest: (a) `parseStatus` accepts/omits `appVersionCode` (non-int → undefined, never NaN); (b) golden vector: `updateToGrant({...}, issuedAt)` serializes byte-stable with all seven body fields; (c) `updateCheck`: manifest versionCode > device → available, ≤ → not, fetch failure → not-available (fail-quiet).
- [ ] **Step 2: Run** — `npm test` → new tests FAIL.
- [ ] **Step 3: Implement** — parse field; builder mirrors `tetheringToGrant`'s shape (kind `"update"`, body from the manifest + `url: "https://charter.mysignet.app/charter-latest.apk"` taken from manifest-relative origin, `packageName: "org.forgesworn.charter"`); Family device card: show `versionName` when current ("Charter <code>"), or the button `Update Charter to <manifest.versionName>` → `policyToClauses`-style publish of the single clause → local "update sent — the phone installs it within a few minutes" state → flips to "Updated ✓" when a later STATUS reports `appVersionCode >= manifest.versionCode`.
- [ ] **Step 4: Run** — `npm run typecheck && npm test` green.
- [ ] **Step 5: Commit** — `feat(mycharter): see device versions + one-tap Update Charter (#44)`.

---

### Task 6: Publish script + version bump

**Files:**
- Create: `android/scripts/publish-apk.sh`
- Modify: `android/app/build.gradle.kts` (versionCode/versionName bump)

**Interfaces:**
- Consumes: `build-jni.sh release`; gradle `assembleDebug`; `apksigner` (SDK build-tools) for the cert digest.
- Produces: `apps/charter-app/public/charter-latest.apk` + `charter-apk.json` `{versionName, versionCode, apkSha256, certSha256, sizeBytes, builtAt}` — the shape Task 5 froze.

- [ ] **Step 1: Script** (complete, fail-closed):

```bash
#!/usr/bin/env bash
# Build + publish the self-update artifact the way charter-latest.deb ships:
# commit APK + manifest into the PWA's public/ and push; the deploy workflow
# serves them at charter.mysignet.app. Requires: source ~/Android/env.sh.
set -euo pipefail
cd "$(dirname "$0")/.."   # android/

./scripts/build-jni.sh release
./gradlew -q assembleDebug
APK=app/build/outputs/apk/debug/app-debug.apk

VC=$(grep -oE 'versionCode = [0-9]+' app/build.gradle.kts | grep -oE '[0-9]+')
VN=$(grep -oE 'versionName = "[^"]+"' app/build.gradle.kts | sed 's/.*"\(.*\)"/\1/')
PUB=../apps/charter-app/public
MANIFEST=$PUB/charter-apk.json

# Forgot-to-bump guard: refuse to republish an already-published versionCode.
if [ -f "$MANIFEST" ]; then
  OLD=$(grep -oE '"versionCode": *[0-9]+' "$MANIFEST" | grep -oE '[0-9]+')
  if [ "$VC" -le "$OLD" ]; then
    echo "FATAL: versionCode $VC <= published $OLD — bump build.gradle.kts" >&2
    exit 1
  fi
fi

SHA=$(sha256sum "$APK" | cut -d' ' -f1)
CERT=$(apksigner verify --print-certs "$APK" \
  | grep -oE 'SHA-256 digest: [0-9a-f]+' | head -1 | awk '{print $3}')
[ -n "$CERT" ] || { echo "FATAL: could not read signing cert digest" >&2; exit 1; }
SIZE=$(stat -c%s "$APK")
BUILT=$(date -u +%Y-%m-%dT%H:%M:%SZ)

cp "$APK" "$PUB/charter-latest.apk"
cat > "$MANIFEST" <<EOF
{
  "versionName": "$VN",
  "versionCode": $VC,
  "apkSha256": "$SHA",
  "certSha256": "$CERT",
  "sizeBytes": $SIZE,
  "builtAt": "$BUILT"
}
EOF
echo "publish: v$VN ($VC) sha=$SHA cert=$CERT size=$SIZE"
echo "Now: git add ../apps/charter-app/public/charter-latest.apk $MANIFEST && commit + push to main."
```

- [ ] **Step 2: Bump** versionCode/versionName in `build.gradle.kts` (next integer + next semver; check current values first).
- [ ] **Step 3: Dry-run the script end-to-end** (build must succeed, manifest values sane, cert digest non-empty, guard triggers on a second run without a bump).
- [ ] **Step 4: Commit** (script + bump only — the built APK/manifest publish to MAIN as part of the rollout step, not this feature branch) — `feat(release): publish-apk script — self-update artifact pipeline (#44)`.

---

### Task 7: Gates, PR, rollout, hardware handoff

- [ ] **Step 1: Full gates** — `linux/` four gates; `cd core && cargo test --workspace`; `android/jni` fmt/clippy/test; `apps/charter-app` typecheck+test; `assembleDebug`.
- [ ] **Step 2: PR** — branch `feat/self-update` → PR referencing #44 with the design summary + what remains hardware-unverified; request decented's 1-click (verify merge actually lands — see memory).
- [ ] **Step 3: Rollout (after merge)** — on `main`: run `publish-apk.sh`, commit APK+manifest, push (deploy workflow ships the PWA + artifact together).
- [ ] **Step 4: Hardware handoff (decented, ~5 min)** — turnkey: open MyCharter → Family → Robin's Pixel card shows "Update Charter to <version>" → tap → within ~3 min the card flips to "Updated ✓". If it does not: re-enable USB debugging for diagnosis (exact toggle steps included in the handoff message).
