#!/usr/bin/env bash
# Build + sign the ward release APK — the ONE build path shared by the local
# ./scripts/publish-apk.sh and the CI release-artifacts.yml workflow, so a
# developer machine and CI are guaranteed to produce the same bytes from the
# same source. Does not upload, announce, or write any manifest.
#
# SIGNING: reads the same material as android/app/build.gradle.kts
# (CHARTER_KEYSTORE_FILE/_PASSWORD, CHARTER_KEY_ALIAS, CHARTER_KEY_PASSWORD,
# or the alpha bridge CHARTER_ALPHA_DEBUG_SIGNING=1 — see
# android/keystore/README.md). This script does not decide signing policy;
# the gradle build does, and fails loudly with neither present.
#
# Key rotation (apksigner v3 proof-of-rotation lineage, C1/C2 of the
# 2026-09-27 rotation plan): the AGP signingConfig block gradle uses has no
# "lineage" field, so a rotation cannot be expressed there. When
# CHARTER_KEYSTORE_FILE names real signing material, CHARTER_SIGNING_LINEAGE_FILE
# is now MANDATORY (android_sign_rotated in lib.sh refuses to build without
# it) — a release-key APK with no lineage installs on no fielded device.
# Under the alpha bridge (no CHARTER_KEYSTORE_FILE) nothing here changes.
#
# Requires: source ~/Android/env.sh (or the CI equivalent: ANDROID_HOME +
# ANDROID_NDK_HOME set, cargo-ndk on PATH, the pinned Rust toolchain + Android
# targets installed — see core/rust-toolchain.toml).
#
# Writes a shell-sourceable file of build facts and prints ONLY that file's
# path on stdout, so a caller does:
#   BUILD_VARS=$(./scripts/build-apk.sh)
#   . "$BUILD_VARS"; rm -f "$BUILD_VARS"
# Vars set: APK (path, relative to android/), VC, VN, CERT, SHA, SIZE, BUILT.
set -euo pipefail
cd "$(dirname "$0")/.."   # android/
# shellcheck source=../../scripts/release/lib.sh
. ../scripts/release/lib.sh

android_resolve_signing_env

if [ -z "${CHARTER_KEYSTORE_FILE:-}" ] && [ "${CHARTER_ALPHA_DEBUG_SIGNING:-}" = "1" ]; then
  echo "WARNING: building a DEBUG-SIGNED ward release (alpha bridge)." >&2
  echo "         Keystore password is the well-known \"android\"; anyone holding" >&2
  echo "         that file can sign a same-signature update of the Device Owner." >&2
  echo "         See android/keystore/README.md." >&2
fi

# Everything a build step prints goes to stderr: the caller does
# BUILD_VARS=$(./scripts/build-apk.sh), and the ONLY stdout line this script
# may ever produce is the final "echo $OUT" below. build-jni.sh's own "OK: …"
# line (and anything gradle prints despite -q) would otherwise land in
# $BUILD_VARS and break `. "$BUILD_VARS"`.
./scripts/build-jni.sh release >&2
./gradlew -q assembleRelease >&2
APK=app/build/outputs/apk/release/app-release.apk
[ -f "$APK" ] || { echo "FATAL: $APK missing after build" >&2; exit 1; }

VC=$(grep -oE 'versionCode = [0-9]+' app/build.gradle.kts | grep -oE '[0-9]+')
VN=$(grep -oE 'versionName = "[^"]+"' app/build.gradle.kts | sed 's/.*"\(.*\)"/\1/')
[ -n "$VC" ] && [ -n "$VN" ] || { echo "FATAL: cannot read version from build.gradle.kts" >&2; exit 1; }

android_sign_rotated "$APK"
CERT=$(android_verify_signing "$APK")
[ -n "$CERT" ] || { echo "FATAL: could not read the signing cert digest" >&2; exit 1; }

SHA=$(sha256sum "$APK" | cut -d' ' -f1)
SIZE=$(stat -c%s "$APK")
BUILT=$(date -u +%Y-%m-%dT%H:%M:%SZ)

OUT=$(mktemp)
{
  echo "APK=$APK"
  echo "VC=$VC"
  echo "VN=$VN"
  echo "CERT=$CERT"
  echo "SHA=$SHA"
  echo "SIZE=$SIZE"
  echo "BUILT=$BUILT"
} >"$OUT"
echo "$OUT"
