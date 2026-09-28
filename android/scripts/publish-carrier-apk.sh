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
# Rotating off the debug cert (android-signing-rotation plan, 2026-09-27) is
# IN PLACE, not a re-install: android_sign_rotated (scripts/release/lib.sh)
# re-signs the build with a v3 lineage from the old debug key plus the new
# release cert (scripts/release/MakeLineage.java), so the phone already
# carrying the debug-signed carrier accepts the rotated update directly, and
# no private key changes hands to build that lineage.
#
# Requires: `source ~/Android/env.sh` (SDK + NDK on PATH).
set -euo pipefail
cd "$(dirname "$0")/.."   # android/
# shellcheck source=../../scripts/release/lib.sh
. ../scripts/release/lib.sh

# The build itself lives in build-carrier-apk.sh — the ONE path CI's
# release-artifacts.yml also runs. See that script for the signing story
# (alpha bridge, lineage rotation).
BUILD_VARS=$(./scripts/build-carrier-apk.sh)
# shellcheck disable=SC1090
. "$BUILD_VARS"
rm -f "$BUILD_VARS"
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

# CERT (and VC/VN/SHA/SIZE/BUILT) came from build-carrier-apk.sh above.

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
