#!/usr/bin/env bash
# Build + publish the Kintrinsic carrier APK (the GUARDIAN-side app, spec D5).
# D3: the APK is uploaded to BLOSSOM, not committed into the repo; only the
# manifest (mycharter-apk.json, naming the Blossom URL) is committed.
#
# Same conventions as the ward artifact, same reasons:
# - `.txt` extension: the vhost's static allowlist has no `apk` yet (#46).
# - Versioned filename: a CDN edge can never serve a stale binary against a
#   new manifest's sha256.
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

# D3: the carrier APK is hosted on BLOSSOM, never committed into this repo.
# Upload + announce FIRST so a failed upload aborts before the manifest names
# a URL that isn't there.
if [ "${CHARTER_RELEASE_EVENT_SKIP:-0}" != "1" ]; then
  URLFILE=$(mktemp)
  ( cd .. && node scripts/release/publish-release.mjs \
      --channel mycharter-apk --artifact "android/$APK" \
      --version "$VN" --version-code "$VC" --cert "$CERT" \
      --emit-url-file "$URLFILE" )
  URL=$(cat "$URLFILE"); rm -f "$URLFILE"
  [ -n "$URL" ] || { echo "FATAL: publisher emitted no verified device URL" >&2; exit 1; }
else
  URL="${CHARTER_BLOSSOM_DL_BASE:-https://nostr.download}/$SHA.apk"
fi

# Origin-JSON fallback (guardian console reads it when relay events are down);
# names the Blossom URL directly. Direct-200 mirror (stagers refuse redirects).
cat > "$MANIFEST" <<EOF
{
  "versionName": "$VN",
  "versionCode": $VC,
  "url": "$URL",
  "apkSha256": "$SHA",
  "certSha256": "$CERT",
  "sizeBytes": $SIZE,
  "builtAt": "$BUILT"
}
EOF
echo "published: mycharter-apk.json → $URL (v$VN, code $VC, sha $SHA)"
echo "next:   commit the manifest and push. The artifact lives on Blossom."
