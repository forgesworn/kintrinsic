# shellcheck shell=bash
# Shared by publish-apk.sh, publish-carrier-apk.sh and publish-deb.sh.
#
# release_publish <repo-root> <channel> <artifact> <version> <version-code> <sha256> [cert]
#
# Runs scripts/release/publish-release.mjs: the artifact goes to its GitHub
# Release first (the primary host, verified end-to-end), then to Blossom as a
# mirror, then the signed kind-30063 event is announced. <artifact> is
# relative to <repo-root>. On success it sets:
#
#   RELEASE_URL        the ONE legacy URL for a manifest's `url` field: the
#                      verified direct-200 Blossom address, because fielded
#                      clients (ward <= 0.6.12, charterd <= 0.7.9, guardian
#                      <= 0.1.14) refuse redirects and GitHub downloads 302.
#   RELEASE_URLS_JSON  the ordered JSON array for a manifest's `urls` field:
#                      GitHub first, then Blossom. Newer clients try each in
#                      turn, following https redirects, pinning the sha256.
#
# CHARTER_RELEASE_EVENT_SKIP=1  offline: nothing uploaded; the deterministic Blossom URL only.
# CHARTER_RELEASE_DRY_RUN=1     pass --dry-run: no network writes, no `gh`.
release_publish() {
  local root=$1 channel=$2 artifact=$3 version=$4 code=$5 sha=$6 cert=${7:-}
  local ext=apk
  [ "$channel" = "charter-deb" ] && ext=deb
  local urlfile urlsfile
  urlfile=$(mktemp)
  urlsfile=$(mktemp)

  if [ "${CHARTER_RELEASE_EVENT_SKIP:-0}" != "1" ]; then
    local extra=()
    [ -n "$cert" ] && extra+=(--cert "$cert")
    [ "${CHARTER_RELEASE_DRY_RUN:-0}" = "1" ] && extra+=(--dry-run)
    if ! ( cd "$root" && node scripts/release/publish-release.mjs \
        --channel "$channel" --artifact "$artifact" \
        --version "$version" --version-code "$code" "${extra[@]}" \
        --emit-url-file "$urlfile" --emit-urls-file "$urlsfile" ); then
      rm -f "$urlfile" "$urlsfile"
      echo "FATAL: publish-release.mjs failed — nothing announced, manifest untouched" >&2
      return 1
    fi
  else
    # Offline build: nothing uploaded or verified — the deterministic Blossom
    # URL only. No GitHub URL: it was never checked, and the front door would
    # make it the download button.
    local blossom="${CHARTER_BLOSSOM_DL_BASE:-https://nostr.download}/$sha.$ext"
    echo "$blossom" >"$urlfile"
    echo "$blossom" >"$urlsfile"
  fi

  RELEASE_URL=$(head -n1 "$urlfile")
  if grep -qvE '^https://[^", ]+$' "$urlsfile"; then
    rm -f "$urlfile" "$urlsfile"
    echo "FATAL: publisher emitted a malformed url list" >&2
    return 1
  fi
  RELEASE_URLS_JSON="[$(sed 's/.*/"&"/' "$urlsfile" | paste -sd, - | sed 's/,/, /g')]"
  rm -f "$urlfile" "$urlsfile"
  case "$RELEASE_URL" in
    https://*) ;;
    *) echo "FATAL: publisher emitted no verified device URL" >&2; return 1 ;;
  esac
  [ "$RELEASE_URLS_JSON" != "[]" ] || { echo "FATAL: publisher emitted no urls" >&2; return 1; }
}

# write_manifest <path> <json>: write it, or in a dry run print it instead so a
# rehearsal never touches the committed manifests.
write_manifest() {
  if [ "${CHARTER_RELEASE_DRY_RUN:-0}" = "1" ]; then
    echo "--dry-run: would write $1:"
    printf '%s\n' "$2"
  else
    printf '%s\n' "$2" >"$1"
  fi
}
