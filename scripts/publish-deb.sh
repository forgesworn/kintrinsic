#!/usr/bin/env bash
# Build + publish the Kintrinsic for Linux .deb. D3: the .deb is uploaded to
# BLOSSOM, not committed into the repo; only the manifest (charter-deb.json,
# naming the Blossom URL) is committed, so the Kintrinsic app can tell a
# guardian their laptop is behind. charterd installs from the relay channel.
#
# Until this existed the deb was copied by hand and no manifest was written at
# all — which is why a paired laptop showed no version and never offered an
# update, while phones did.
#
# Requires the Linux toolchain (see linux/README.md). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."          # repo root
PUB=apps/charter-app/public
MANIFEST=$PUB/charter-deb.json

VN=$(grep -m1 -oE '^version = "[^"]+"' linux/Cargo.toml | sed 's/.*"\(.*\)"/\1/')
[ -n "$VN" ] || { echo "FATAL: cannot read version from linux/Cargo.toml" >&2; exit 1; }

# major.minor.patch -> major*10000 + minor*100 + patch, lanes saturating at 99.
# MUST match charterd's version::parse_version_code, or the guardian is told a
# laptop is behind when it isn't (or worse, offered a downgrade).
IFS=. read -r MA MI PA <<<"${VN%%[-+]*}"
MI=$(( ${MI:-0} > 99 ? 99 : ${MI:-0} ))
PA=$(( ${PA:-0} > 99 ? 99 : ${PA:-0} ))
VC=$(( ${MA:-0} * 10000 + MI * 100 + PA ))

echo "building kintrinsic $VN (code $VC)…"
( cd linux && cargo run -q -p xtask -- deb >/dev/null )
DEB="linux/target/deb/kintrinsic_${VN}_amd64.deb"
[ -f "$DEB" ] || { echo "FATAL: $DEB missing after build" >&2; exit 1; }

# Forgot-to-bump guard: refuse to republish an already-published version.
if [ -f "$MANIFEST" ]; then
  OLD=$(grep -oE '"versionCode": *[0-9]+' "$MANIFEST" | grep -oE '[0-9]+' || true)
  if [ -n "$OLD" ] && [ "$VC" -le "$OLD" ]; then
    echo "FATAL: versionCode $VC <= published $OLD — bump linux/Cargo.toml first" >&2
    exit 1
  fi
fi

SHA=$(sha256sum "$DEB" | cut -d' ' -f1)
SIZE=$(stat -c%s "$DEB")
BUILT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
# D3: the .deb is hosted on BLOSSOM, not committed into this repo. Upload +
# announce + verify the exact device URL FIRST; a failure aborts (set -e)
# before the manifest claims a URL. charterd installs it from the relay
# channel; the front door + this manifest reference the verified Blossom URL.
if [ "${CHARTER_RELEASE_EVENT_SKIP:-0}" != "1" ]; then
  URLFILE=$(mktemp)
  node scripts/release/publish-release.mjs \
    --channel charter-deb --artifact "$DEB" \
    --version "$VN" --version-code "$VC" \
    --emit-url-file "$URLFILE"
  URL=$(cat "$URLFILE"); rm -f "$URLFILE"
  [ -n "$URL" ] || { echo "FATAL: publisher emitted no verified device URL" >&2; exit 1; }
else
  URL="${CHARTER_BLOSSOM_DL_BASE:-https://nostr.download}/$SHA.deb"
fi

cat > "$MANIFEST" <<EOF
{
  "versionName": "$VN",
  "versionCode": $VC,
  "url": "$URL",
  "sha256": "$SHA",
  "sizeBytes": $SIZE,
  "builtAt": "$BUILT"
}
EOF
echo "published: charter-deb.json → $URL (v$VN, code $VC, sha $SHA)"
echo "next: ./scripts/sync-front-door-downloads.sh, then commit the manifests + site/ and push."
