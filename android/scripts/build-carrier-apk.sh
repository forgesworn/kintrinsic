#!/usr/bin/env bash
# Build + sign the Kintrinsic carrier (guardian) release APK — the ONE build
# path shared by the local ./scripts/publish-carrier-apk.sh and the CI
# release-artifacts.yml workflow. Does not upload, announce, or write any
# manifest. Same conventions and signing story as build-apk.sh (the ward
# script); see there for the lineage-rotation note.
#
# Requires: source ~/Android/env.sh (or the CI equivalent).
#
# Key rotation: CHARTER_SIGNING_LINEAGE_FILE is mandatory whenever
# CHARTER_KEYSTORE_FILE names real signing material — see build-apk.sh and
# scripts/release/lib.sh (android_sign_rotated / android_verify_signing).
#
# Writes a shell-sourceable file of build facts and prints ONLY that file's
# path on stdout:
#   BUILD_VARS=$(./scripts/build-carrier-apk.sh)
#   . "$BUILD_VARS"; rm -f "$BUILD_VARS"
# Vars set: APK (path, relative to android/), VC, VN, CERT, SHA, SIZE, BUILT.
set -euo pipefail
cd "$(dirname "$0")/.."   # android/
# shellcheck source=../../scripts/release/lib.sh
. ../scripts/release/lib.sh

android_resolve_signing_env

if [ -z "${CHARTER_KEYSTORE_FILE:-}" ] && [ "${CHARTER_ALPHA_DEBUG_SIGNING:-}" = "1" ]; then
  echo "WARNING: building a DEBUG-SIGNED Kintrinsic release (alpha bridge)." >&2
  echo "         This app holds the guardian secret key. Keystore password is the" >&2
  echo "         well-known \"android\". See android/keystore/README.md." >&2
fi

# D1: the console is BUNDLED into this APK (gradle stageConsoleAssets), so the
# APK must always carry a freshly built page — a stale dist/ here would ship
# an old console under a new versionCode.
#
# Everything a build step prints goes to stderr: the caller does
# BUILD_VARS=$(./scripts/build-carrier-apk.sh), and the ONLY stdout line this
# script may ever produce is the final "echo $OUT" below. `npm run build`,
# build-jni-guardian.sh's own "OK: …" line, and anything gradle prints
# despite -q would otherwise land in $BUILD_VARS and break `. "$BUILD_VARS"`.
( cd ../apps/charter-app && npm run build ) >&2

./scripts/build-jni-guardian.sh release >&2
./gradlew -q :carrier:assembleRelease >&2
APK=carrier/build/outputs/apk/release/carrier-release.apk
[ -f "$APK" ] || { echo "FATAL: $APK missing after build" >&2; exit 1; }

VC=$(grep -oE 'versionCode = [0-9]+' carrier/build.gradle.kts | grep -oE '[0-9]+')
VN=$(grep -oE 'versionName = "[^"]+"' carrier/build.gradle.kts | sed 's/.*"\(.*\)"/\1/')
[ -n "$VC" ] && [ -n "$VN" ] || { echo "FATAL: cannot read version from carrier/build.gradle.kts" >&2; exit 1; }

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
