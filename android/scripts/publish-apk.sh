#!/usr/bin/env bash
# Build + publish the ward self-update artifact (#44). The APK is NOT committed
# into the repo: it goes to its GitHub Release (tag ward-v<version>, the
# primary host) and to BLOSSOM as a mirror; only the tiny manifest
# (charter-apk.json) is committed. The signed relay release event is the
# primary update channel; this manifest is the console's fallback when relays
# are unreachable. See scripts/release/lib.sh for the url/urls split.
#
# Rehearsal (builds, signs a throwaway event, uploads nothing, writes no
# manifest): CHARTER_RELEASE_DRY_RUN=1 ./scripts/publish-apk.sh
#
# SIGNING (S3, review 2026-08-07). This publishes the artifact a Device Owner
# installs over itself, so the key it is signed with is the whole trust story.
# The build now REFUSES to fall back to the debug key on its own; supply real
# material (`CHARTER_KEYSTORE_*`, the sysadmin's, normally injected in CI) or type
# the bridge out in full:
#
#     CHARTER_ALPHA_DEBUG_SIGNING=1 ./scripts/publish-apk.sh
#
# The bridge exists because deployed phones are pinned to the debug cert and
# Android only accepts same-signature updates — rotating means a one-time
# re-pin on every phone in the field. It is not "on purpose", it is a debt,
# and it is spelled out at every use so it can't be forgotten.
#
# Requires: `source ~/Android/env.sh` (SDK + NDK on PATH).
set -euo pipefail
cd "$(dirname "$0")/.."   # android/
# shellcheck source=../../scripts/release/lib.sh
. ../scripts/release/lib.sh

if [ -z "${CHARTER_KEYSTORE_FILE:-}" ] && [ "${CHARTER_ALPHA_DEBUG_SIGNING:-}" = "1" ]; then
  echo "WARNING: publishing a DEBUG-SIGNED ward release (alpha bridge)." >&2
  echo "         Keystore password is the well-known \"android\"; anyone holding" >&2
  echo "         that file can sign a same-signature update of the Device Owner." >&2
  echo "         See android/keystore/README.md." >&2
fi

./scripts/build-jni.sh release
./gradlew -q assembleRelease
APK=app/build/outputs/apk/release/app-release.apk
[ -f "$APK" ] || { echo "FATAL: $APK missing after build" >&2; exit 1; }

VC=$(grep -oE 'versionCode = [0-9]+' app/build.gradle.kts | grep -oE '[0-9]+')
VN=$(grep -oE 'versionName = "[^"]+"' app/build.gradle.kts | sed 's/.*"\(.*\)"/\1/')
[ -n "$VC" ] && [ -n "$VN" ] || { echo "FATAL: cannot read version from build.gradle.kts" >&2; exit 1; }
PUB=../apps/charter-app/public
MANIFEST=$PUB/charter-apk.json

# Forgot-to-bump guard: refuse to republish an already-published versionCode.
if [ -f "$MANIFEST" ]; then
  OLD=$(grep -oE '"versionCode": *[0-9]+' "$MANIFEST" | grep -oE '[0-9]+')
  if [ -n "$OLD" ] && [ "$VC" -le "$OLD" ]; then
    echo "FATAL: versionCode $VC <= published $OLD — bump app/build.gradle.kts first" >&2
    exit 1
  fi
fi

APKSIGNER=$(ls "$ANDROID_HOME"/build-tools/*/apksigner 2>/dev/null | sort -V | tail -1)
[ -n "$APKSIGNER" ] || { echo "FATAL: apksigner not found under \$ANDROID_HOME/build-tools" >&2; exit 1; }
CERT=$("$APKSIGNER" verify --print-certs "$APK" \
  | grep -oiE 'SHA-256 digest: [0-9a-f]+' | head -1 | awk '{print tolower($3)}')
[ -n "$CERT" ] || { echo "FATAL: could not read the signing cert digest" >&2; exit 1; }

# The App Link anchor must name THIS cert (S3 item 5). `assetlinks.json` is
# what tells Android that org.forgesworn.charter may handle charter.mysignet.app
# links — the verified App Link the one-scan QR pairing rides on. It carries a
# SHA-256 cert fingerprint, and nothing has ever checked that the fingerprint
# matched the key we actually ship with; it silently held a developer laptop's
# debug cert. A mismatch does not fail loudly on a phone, it just quietly stops
# verifying, and pairing "mysteriously" opens a browser instead of the app.
#
# So: compare, every publish. On a keystore rotation this fails until someone
# updates assetlinks.json with the new fingerprint, which is exactly the
# reminder that rotation needs.
ASSETLINKS=$PUB/.well-known/assetlinks.json
if [ -f "$ASSETLINKS" ]; then
  # Colon-separated uppercase (the assetlinks form) from the digest we read above.
  WANT=$(echo "$CERT" | tr 'a-z' 'A-Z' | sed 's/../&:/g; s/:$//')
  if ! grep -qF "$WANT" "$ASSETLINKS"; then
    echo "FATAL: assetlinks.json does not list this APK's signing certificate." >&2
    echo "  APK cert SHA-256: $WANT" >&2
    echo "  $ASSETLINKS lists:" >&2
    grep -oE '"[0-9A-F:]{95}"' "$ASSETLINKS" | sed 's/^/    /' >&2
    echo "  Verified App Links (and one-scan QR pairing) break silently on a" >&2
    echo "  mismatch. Update assetlinks.json with the fingerprint above — see" >&2
    echo "  android/keystore/README.md." >&2
    exit 1
  fi
fi

SHA=$(sha256sum "$APK" | cut -d' ' -f1)
SIZE=$(stat -c%s "$APK")
BUILT=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# GitHub Release first, Blossom mirror second, then the signed event — a
# failed GitHub upload or download check aborts (set -e) before anything is
# announced or the manifest names a URL.
release_publish .. charter-apk "android/$APK" "$VN" "$VC" "$SHA" "$CERT"

# The origin-JSON manifest is the guardian console's FALLBACK when the signed
# relay release events are unreachable. `url` stays the direct-200 Blossom
# address fielded guardians (<= 0.1.14) hand to wards that refuse redirects;
# `urls` is the ordered list (GitHub first) newer consoles read.
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

echo "published: charter-apk.json → $RELEASE_URLS_JSON"
echo "  version: $VN ($VC)   size: $SIZE bytes   apkSha256: $SHA   certSha256: $CERT"
echo "Next: commit the manifest (apps/charter-app/public/charter-apk.json) and push."
echo "The artifact lives on GitHub Releases (ward-v$VN) and Blossom, not in the repo."
