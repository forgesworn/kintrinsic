# Charter self-update on a chartered phone (issue #44)

**Date:** 2026-07-21 · **Approved:** decented (in-session) · **Scope:** v1, guardian-initiated

> **D2 note (2026-08-12):** the manifest half of this design is superseded —
> the guardian console now learns releases from SIGNED kind-30063 relay
> events (pinned release key) with the origin `/charter-apk.json` as unsigned
> fallback, and the clause's `url` is a Blossom mirror. The on-device halves
> (clause verify, `UrlStager`, cert gate, `PackageInstaller`) are unchanged
> and still authoritative as written. See
> `docs/superpowers/plans/2026-08-12-d2-release-events.md`.

## Problem

A chartered phone's Device-Owner baseline (`DISALLOW_INSTALL_APPS` +
`DISALLOW_INSTALL_UNKNOWN_SOURCES`) blocks every APK install — including
`adb install -r` of Charter itself. Removing the Device Owner to update is
destructive (re-provisioning needs an account-free device). Deployed phones
are therefore frozen at their install-time app version. Found live 2026-07-21
trying to ship PR #43 to the first family device.

The primitive already exists: the guardian-signed `install.apk` grant +
`DpmApkInstallOps` (staged file, pinned signing-cert SHA-256, silent DO
PackageInstaller commit). It cannot yet target Charter itself: the only
`ApkSource` is `staged` (a parent-writable local file), there is no way to get
bytes onto a locked phone, and the guardian cannot see device app versions.

## Decisions (with decented)

1. **Guardian-initiated** updates: MyCharter shows "update available", the
   guardian taps, a fresh guardian-signed grant authorizes exactly one
   version. No standing authorization / auto-update in v1.
2. **Artifact path v1 = laptop publish script → site**, mirroring
   `charter-latest.deb`: build on the laptop (where the dev keystore that the
   deployed phone trusts lives), commit APK + manifest into
   `apps/charter-app/public/`, push; the existing deploy workflow serves them
   at charter.mysignet.app. CI builds + the sysadmin's release keystore are
   follow-ups, out of scope.

## Components

### 1. Publish script — `android/scripts/publish-apk.sh`
- `build-jni.sh release` (release-profile Rust, real-relay only, mock-leak
  gate) then `assembleDebug` (debug-signed = signature continuity with the
  deployed phone). The published artifact keeps both shipping ABIs
  (arm64-v8a + x86_64) — one artifact for real phones and the CI/emulator.
- Emits `apps/charter-app/public/charter-latest.apk` +
  `charter-apk.json`: `{versionName, versionCode, apkSha256, certSha256,
  sizeBytes, builtAt}` (cert fp read from the APK itself via apksigner).
- Refuses to publish if `versionCode` ≤ the currently published manifest's
  (forgot-to-bump guard). Commits + pushes to main (deploy workflow path
  filter `apps/charter-app/**` already covers it).

### 2. Wire (additive, backward-compatible)
- **Correction during implementation:** self-update is a **clause**, not an
  `install.apk` grant — grants are request-bound (they must echo a
  device-originated request's reqId + nonce, and no request exists for a
  guardian-initiated update). New `ClauseKind::Update` (serde `"update"`,
  store_key 8; routed to the sole ward like `tethering`) with body
  `UpdateAppBody { v, packageName, versionCode, versionName, url, apkSha256,
  signerCertSha256 }`, validated fail-closed (https-only, valid package name,
  versionCode > 0). Level-triggered: the device converges whenever behind.
- `StatusPayload` gains `app_version_code: Option<u64>`
  (`skip_serializing_if none` — old readers unaffected; STATUS_VERSION stays 1).
  Kotlin supplies its `BuildConfig.VERSION_CODE` to the JNI warden at init.

### 3. Phone (Kotlin + JNI)
- `PendingInstall` carries `url` + `apk_sha256` through the queue.
- New `UrlStager` (slow worker): downloads to the existing
  `stagingDir/<packageName>.apk`, streams SHA-256 during download, compares to
  the grant's pin. Mismatch → delete + `TERMINAL`. Network failure →
  `TRANSIENT` (redriven next poll; the 7-day directive sweep bounds retries).
  On success, hands to the existing staged path (which independently
  re-verifies signing-cert continuity before commit — two pins, both must hold).
- Self-update semantics: the DO PackageInstaller commit kills our process;
  the system completes the session; BootReceiver/sticky-service restart
  brings the warden back (state is in device-protected storage); the existing
  version-idempotence ("already ≥ target → OK") clears the redriven directive.
  No new code needed beyond the stager — verified on-metal.

### 4. MyCharter (PWA)
- Device card reads `appVersionCode` from STATUS; fetches `/charter-apk.json`
  (same-origin). Behind → "Update Charter to <versionName>" button.
- Tap → issues the guardian-signed `install.apk` grant for
  `org.forgesworn.charter` `{versionCode, source: "url", url, apkSha256,
  signerCertSha256}` via the existing local signer, same as other clauses.
- Progress is observational: STATUS `appVersionCode` flips within ~2 min of
  install (60 s heartbeat + poll lag) → "Updated ✓". A "sent — the phone
  updates within a few minutes" state covers the gap; no new wire needed.

### 5. Testing
- **Rust (proto/jni):** grant validation (url+sha required/https-only/absent
  for staged), queue round-trip with the new fields, hash-mismatch →
  TERMINAL, version idempotence (exists).
- **Kotlin:** UrlStager hash-verify + failure mapping against a local HTTP
  fixture (existing test harness style).
- **PWA (vitest):** version compare, grant issuance payload shape.
- **Gates:** core four gates (from `linux/`), `cargo test/clippy/fmt` in
  `android/jni`, PWA `typecheck + test`, `assembleDebug` compiles.
- **On-metal (decented, ~5 min):** publish for real → MyCharter shows update →
  tap → phone self-updates → MyCharter shows new version. USB debugging stays
  off; adb only as failure fallback.

## Out of scope
CI-built artifacts, the sysadmin release keystore + re-provision story,
auto-update policy, fleet rollout UI, downgrade support (Android forbids
downgrades; the version guard makes it explicit).
