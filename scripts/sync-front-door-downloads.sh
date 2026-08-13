#!/usr/bin/env bash
# Point the front-door download page (kintrinsic.app, the site/ directory) at
# the current release artifacts on BLOSSOM. Artifacts are NOT copied into the
# repo any more (D3 decoupling): they are content-addressed blobs the publish
# scripts upload to Blossom, and this writes their URLs into downloads.json,
# which download.html reads to set the download buttons.
#
# Run AFTER the publish scripts (which upload to Blossom + write the manifests
# with the artifact sha256). Then commit + push this repo — the deploy-site
# workflow ships site/ on push to main. This script only stages downloads.json;
# it pushes nothing and copies no binaries.
#
# Blossom download base for the browser: nostr.download serves `/<sha>.<ext>`
# with the right content-type (verified 2026-08-12). Override with
# CHARTER_BLOSSOM_DL_BASE.
set -euo pipefail
cd "$(dirname "$0")/.."          # charter repo root
PUB=apps/charter-app/public
CY="${CHARTER_YOU_DIR:-./site}"
BASE="${CHARTER_BLOSSOM_DL_BASE:-https://nostr.download}"

[ -d "$CY" ] || { echo "front-door site dir not found at $CY (set CHARTER_YOU_DIR)"; exit 1; }

# The two user-facing artifacts: the guardian carrier APK (Android) and the
# Linux deb. Take the *verified* device URL straight from the manifest the
# publish script wrote (it verified that exact URL direct-200) — do NOT
# reconstruct it, so a partial-mirror release can't produce a dead front-door
# link (the sysadmin, 2026-08-12).
field() { grep -oE "\"$2\": *\"[^\"]+\"" "$1" | head -1 | sed -E "s/\"$2\": *\"([^\"]+)\"/\1/"; }

AVER=$(field "$PUB/mycharter-apk.json" versionName)
AURL=$(field "$PUB/mycharter-apk.json" url)
ASHA=$(field "$PUB/mycharter-apk.json" apkSha256)
LVER=$(field "$PUB/charter-deb.json" versionName)
LURL=$(field "$PUB/charter-deb.json" url)
LSHA=$(field "$PUB/charter-deb.json" sha256)

for pair in "$AVER" "$AURL" "$ASHA" "$LVER" "$LURL" "$LSHA"; do
  [ -n "$pair" ] || { echo "FATAL: a manifest is missing version/url/sha — run the publish scripts first"; exit 1; }
done

cat > "$CY/downloads.json" <<EOF
{
  "android": {
    "version": "$AVER",
    "url": "$AURL",
    "sha256": "$ASHA"
  },
  "linux": {
    "version": "$LVER",
    "url": "$LURL",
    "sha256": "$LSHA"
  }
}
EOF

echo "downloads.json → android $AVER ($AURL)"
echo "                 linux   $LVER ($LURL)"
echo
echo "Also update the no-JS fallback hrefs in site/download.html (data-dl=android|linux)"
echo "to the two URLs above, then commit site/ and push (deploy-site ships it)."
echo "Verify the blobs resolve first: curl -sI $LURL | head -1"
