#!/usr/bin/env bash
# Build + publish the Kintrinsic carrier APK (the GUARDIAN-side app, spec D5).
# The APK is not committed into the repo: it goes to its GitHub Release (tag
# guardian-v<version>, the primary host, asset kintrinsic-<version>.apk) and to
# BLOSSOM as a mirror; only the manifest (mycharter-apk.json) is committed.
# Same conventions as the ward artifact (publish-apk.sh), same reasons.
#
# Rehearsal (builds, signs a throwaway event, uploads nothing, writes no
# manifest): CHARTER_RELEASE_DRY_RUN=1 ./scripts/publish-carrier-apk.sh
#
# SIGNING (S3, review 2026-08-07). This app holds the GUARDIAN SECRET KEY, so
# its signing key is the family's root of trust twice over. The carrier used to
# be debug-signed unconditionally with no override path at all; it now reads
# the same `CHARTER_KEYSTORE_*` material as the ward app and refuses to build a
# release without it. For the phones already pinned to the debug cert, the
# bridge has to be typed out in full:
#
#     CHARTER_ALPHA_DEBUG_SIGNING=1 ./scripts/publish-carrier-apk.sh
#
# Requires: `source ~/Android/env.sh` (SDK + NDK on PATH).
set -euo pipefail
cd "$(dirname "$0")/.."   # android/
# shellcheck source=../../scripts/release/lib.sh
. ../scripts/release/lib.sh

if [ -z "${CHARTER_KEYSTORE_FILE:-}" ] && [ "${CHARTER_ALPHA_DEBUG_SIGNING:-}" = "1" ]; then
  echo "WARNING: publishing a DEBUG-SIGNED Kintrinsic release (alpha bridge)." >&2
  echo "         This app holds the guardian secret key. Keystore password is the" >&2
  echo "         well-known \"android\". See android/keystore/README.md." >&2
fi

# D1: the console is BUNDLED into this APK (gradle stageConsoleAssets), so the
# APK must always carry a freshly built page — a stale dist/ here would ship
# an old console under a new versionCode.
( cd ../apps/charter-app && npm run build )

./scripts/build-jni-guardian.sh release
./gradlew -q :carrier:assembleRelease
APK=carrier/build/outputs/apk/release/carrier-release.apk
[ -f "$APK" ] || { echo "FATAL: $APK missing after build" >&2; exit 1; }

VC=$(grep -oE 'versionCode = [0-9]+' carrier/build.gradle.kts | grep -oE '[0-9]+')
VN=$(grep -oE 'versionName = "[^"]+"' carrier/build.gradle.kts | sed 's/.*"\(.*\)"/\1/')
[ -n "$VC" ] && [ -n "$VN" ] || { echo "FATAL: cannot read version from carrier/build.gradle.kts" >&2; exit 1; }
PUB=../apps/charter-app/public
MANIFEST=$PUB/mycharter-apk.json

# Forgot-to-bump guard: refuse to republish an already-published versionCode.
if [ -f "$MANIFEST" ]; then
  OLD=$(grep -oE '"versionCode": *[0-9]+' "$MANIFEST" | grep -oE '[0-9]+')
  if [ -n "$OLD" ] && [ "$VC" -le "$OLD" ]; then
    echo "FATAL: versionCode $VC <= published $OLD — bump carrier/build.gradle.kts first" >&2
    exit 1
  fi
fi

APKSIGNER=$(ls "$ANDROID_HOME"/build-tools/*/apksigner 2>/dev/null | sort -V | tail -1)
[ -n "$APKSIGNER" ] || { echo "FATAL: apksigner not found under \$ANDROID_HOME/build-tools" >&2; exit 1; }
CERT=$("$APKSIGNER" verify --print-certs "$APK" \
  | grep -oiE 'SHA-256 digest: [0-9a-f]+' | head -1 | awk '{print tolower($3)}')
[ -n "$CERT" ] || { echo "FATAL: could not read the signing cert digest" >&2; exit 1; }

SHA=$(sha256sum "$APK" | cut -d' ' -f1)
SIZE=$(stat -c%s "$APK")
BUILT=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# GitHub Release first, Blossom mirror second, then the signed event; any
# GitHub failure aborts before the manifest names a URL.
release_publish .. mycharter-apk "android/$APK" "$VN" "$VC" "$SHA" "$CERT"

# Origin-JSON fallback (guardian console reads it when relay events are down).
# `url`: the direct-200 Blossom address for redirect-refusing fielded shells;
# `urls`: GitHub first, then Blossom.
write_manifest "$MANIFEST" "{
  \"versionName\": \"$VN\",
  \"versionCode\": $VC,
  \"url\": \"$RELEASE_URL\",
  \"urls\": $RELEASE_URLS_JSON,
  \"apkSha256\": \"$SHA\",
  \"certSha256\": \"$CERT\",
  \"sizeBytes\": $SIZE,
  \"builtAt\": \"$BUILT\"
}"
echo "published: mycharter-apk.json → $RELEASE_URLS_JSON (v$VN, code $VC, sha $SHA)"
echo "next:   commit the manifest and push. The artifact lives on GitHub Releases"
echo "        (guardian-v$VN) and Blossom."
