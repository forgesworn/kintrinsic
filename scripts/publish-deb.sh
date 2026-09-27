#!/usr/bin/env bash
# Build + publish the Kintrinsic for Linux .deb. The .deb is not committed into
# the repo: it goes to its GitHub Release (tag linux-v<version>, the primary
# host) and to BLOSSOM as a mirror; only the manifest (charter-deb.json) is
# committed, so the Kintrinsic app can tell a guardian their laptop is behind.
# charterd learns of it from the signed relay event and stages it itself.
#
# Rehearsal (builds, signs a throwaway event, uploads nothing, writes no
# manifest): CHARTER_RELEASE_DRY_RUN=1 ./scripts/publish-deb.sh
#
# Until this existed the deb was copied by hand and no manifest was written at
# all — which is why a paired laptop showed no version and never offered an
# update, while phones did.
#
# Requires the Linux toolchain (see linux/README.md). Run from anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."          # repo root
# shellcheck source=release/lib.sh
. scripts/release/lib.sh
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
# GitHub Release first (verified end-to-end), Blossom mirror second, then the
# signed event; a GitHub failure aborts (set -e) before the manifest claims a
# URL. `url` stays the direct-200 Blossom address (charterd <= 0.7.9 refuses
# redirects); `urls` leads with GitHub for the front door and newer clients.
release_publish . charter-deb "$DEB" "$VN" "$VC" "$SHA"

write_manifest "$MANIFEST" "{
  \"versionName\": \"$VN\",
  \"versionCode\": $VC,
  \"url\": \"$RELEASE_URL\",
  \"urls\": $RELEASE_URLS_JSON,
  \"sha256\": \"$SHA\",
  \"sizeBytes\": $SIZE,
  \"builtAt\": \"$BUILT\"
}"
echo "published: charter-deb.json → $RELEASE_URLS_JSON (v$VN, code $VC, sha $SHA)"
echo "next: ./scripts/sync-front-door-downloads.sh, then commit the manifests + site/ and push."
