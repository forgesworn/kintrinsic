# shellcheck shell=bash
# Shared by publish-apk.sh, publish-carrier-apk.sh, publish-deb.sh, and the
# android/scripts/build-*.sh scripts (which CI's release-artifacts.yml also
# calls).
#
# The fielded debug certificate (androiddebugkey, ~/.android/debug.keystore,
# store/key pass "android"). Every ward/guardian phone shipped before the
# 2026-09-27 signing-key rotation is signed with this cert; it is also
# apps/charter-app/public/.well-known/assetlinks.json's fingerprint (minus
# colons, lowercased) and the CERT fixture in release-helpers.test.mjs.
# Public data — safe to hard-code; it is what a rotation lineage must START
# FROM, never trust-material of its own.
DEBUG_CERT_SHA256=d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42

# The pinned release-key certificate fingerprint (lowercase hex, 64 chars).
# Pinned to the sysadmin's release cert (android-signing-rotation plan,
# 2026-09-27): every real-signing build's current signer must equal it
# exactly, in both this file and scripts/release/release-helpers.mjs's own
# RELEASE_CERT_SHA256 (kept as two copies for the same reason
# DEBUG_CERT_SHA256 is: one is bash, one is JS). See docs/releasing.md.
RELEASE_CERT_SHA256=4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc
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

# Resolve the newest apksigner under $ANDROID_HOME/build-tools.
_android_apksigner() {
  local apksigner
  apksigner=$(ls "$ANDROID_HOME"/build-tools/*/apksigner 2>/dev/null | sort -V | tail -1)
  [ -n "$apksigner" ] || { echo "FATAL: apksigner not found under \$ANDROID_HOME/build-tools" >&2; return 1; }
  echo "$apksigner"
}

# android_resolve_signing_env
#
# Release signing material may come from real env vars (CI's own path) or
# from a git-ignored android/signing.properties (a one-off local release
# build) — see android/app/build.gradle.kts's signingVal(): env wins when
# both exist, and it reads CHARTER_KEYSTORE_FILE/_PASSWORD, CHARTER_KEY_ALIAS
# and CHARTER_KEY_PASSWORD from either source. android_sign_rotated and
# android_verify_signing below key off the CHARTER_KEYSTORE_FILE env var
# alone: if signing.properties supplies the keystore but nothing ever
# exports CHARTER_KEYSTORE_FILE as an env var, Gradle happily signs a REAL
# release key while these functions believe it's the alpha bridge —
# silently skipping the mandatory rotation lineage and the v3/cert guards
# entirely. So: whenever CHARTER_KEYSTORE_FILE is unset but
# android/signing.properties names a storeFile, export the same four vars
# from it BEFORE gradle runs, so every later check sees exactly what Gradle
# is about to sign with.
#
# _properties_get <file> <key>
#
# A Java-Properties-shaped read of one key, close enough for
# android/signing.properties: `key=value`, `key = value`, `key: value` and
# whitespace-only separators are all accepted (leading/trailing whitespace
# around the value is trimmed); a trailing `\r` (CRLF file) is stripped from
# every line first; a line whose first non-blank character is `#` or `!` is
# a comment. Later duplicate keys win, matching java.util.Properties.load's
# hashtable-put semantics. Does NOT implement backslash/unicode escapes or
# line continuations — signing.properties has never used either. Prints the
# value and exits 0 if the key was found (even if the value is empty), exits
# 1 with no output if it was not.
_properties_get() {
  local file=$1 key=$2
  awk -v key="$key" '
    {
      line = $0
      sub(/\r$/, "", line)
      trimmed = line
      sub(/^[ \t]+/, "", trimmed)
      if (trimmed == "" || trimmed ~ /^[#!]/) next
      if (match(trimmed, /^[^=: \t]+/)) {
        k = substr(trimmed, RSTART, RLENGTH)
        rest = substr(trimmed, RSTART + RLENGTH)
        sub(/^[ \t]*[:=][ \t]*/, "", rest)
        sub(/^[ \t]+/, "", rest)
        sub(/[ \t]+$/, "", rest)
        if (k == key) { val = rest; found = 1 }
      }
    }
    END { if (found) { print val; exit 0 } else exit 1 }
  ' "$file"
}

# Must be called from inside android/ (build-apk.sh / build-carrier-apk.sh
# both `cd` there first) — signing.properties lives at that path, and a
# relative storeFile inside it resolves the same way Gradle's
# rootProject.file() does, from that same directory.
#
# If the file exists but names a storeFile, all four keys (storeFile,
# storePassword, keyAlias, keyPassword) must be present and non-empty — a
# partially-readable config is refused loudly rather than silently exporting
# blanks for the missing ones (which apksigner would then fail on with a
# confusing error, or Gradle might not fail on at all). A file with no
# storeFile at all is treated the same as no file: this isn't configuring
# real signing, so the alpha bridge / packageRelease refusal below is
# unchanged.
android_resolve_signing_env() {
  [ -n "${CHARTER_KEYSTORE_FILE:-}" ] && return 0   # env already wins; nothing to resolve
  local props=signing.properties
  [ -f "$props" ] || return 0   # no file either — alpha bridge or refusal, unchanged

  local store_file store_password key_alias key_password
  store_file=$(_properties_get "$props" storeFile) || store_file=""
  [ -n "$store_file" ] || return 0   # file exists but doesn't name real signing material

  store_password=$(_properties_get "$props" storePassword) || store_password=""
  key_alias=$(_properties_get "$props" keyAlias) || key_alias=""
  key_password=$(_properties_get "$props" keyPassword) || key_password=""
  if [ -z "$store_password" ] || [ -z "$key_alias" ] || [ -z "$key_password" ]; then
    echo "FATAL: $props names storeFile but is missing storePassword/keyAlias/keyPassword (or one is empty)." >&2
    echo "  Refusing a partial signing config rather than exporting blanks for the rest." >&2
    return 1
  fi

  export CHARTER_KEYSTORE_FILE="$store_file"
  export CHARTER_KEYSTORE_PASSWORD="$store_password"
  export CHARTER_KEY_ALIAS="$key_alias"
  export CHARTER_KEY_PASSWORD="$key_password"
  echo "signing: using android/signing.properties for release material (CHARTER_KEYSTORE_FILE was not exported) — rotation lineage and cert guards apply as usual" >&2
}

# android_sign_rotated <apk>
#
# Re-signs a gradle-produced release APK in place with the release key's v3
# proof-of-rotation lineage (android-signing-rotation plan, 2026-09-27). AGP's
# signingConfig has no lineage field, so this is a mandatory post-build
# apksigner step whenever CHARTER_KEYSTORE_FILE names real signing material:
# a release-key APK with NO rotation lineage installs on NO fielded device
# (every one is still on the debug cert) — it would pass every on-device
# gate (ApkInstallOps etc. only check the cert the RELEASE names) and fail
# only at the install commit (INSTALL_FAILED_UPDATE_INCOMPATIBLE). So this
# refuses to build one.
#
# Under the alpha bridge (no CHARTER_KEYSTORE_FILE) this is a no-op —
# behaviour there is unchanged.
#
# v1/v2 are switched OFF and v3 ON: apksigner refuses to sign v1/v2 with a
# lineage present unless the debug signer is re-added under those legacy
# schemes too, which would make it a valid signer again under v1/v2 — v3-only
# is the simple, correct shape. --rotation-min-sdk-version 28 (not the v3.1
# default, 33): the carrier's minSdk is 29, and apksigner refuses the v3.1
# default there ("ensure a signer targets SDK version 29"); 28 covers both
# apps' minSdk uniformly.
android_sign_rotated() {
  local apk=$1
  [ -n "${CHARTER_KEYSTORE_FILE:-}" ] || return 0   # alpha bridge: unchanged
  if [ -z "${RELEASE_CERT_SHA256:-}" ]; then
    # Fail HERE, before signing anything: the real-signing path with no pin
    # yet would still produce a draft that android_verify_signing's own pin
    # check (or --from-draft's assertPinnedCert) refuses to ever publish —
    # better to never build it, so CI fails fast instead of landing a draft
    # nobody can flip to published.
    echo "FATAL: release cert not pinned yet — see docs/releasing.md" >&2
    return 1
  fi
  if [ -z "${CHARTER_SIGNING_LINEAGE_FILE:-}" ]; then
    echo "FATAL: CHARTER_KEYSTORE_FILE is set but CHARTER_SIGNING_LINEAGE_FILE is not." >&2
    echo "  A release-key APK with no rotation lineage installs on NO fielded device" >&2
    echo "  (every one is still on the debug cert) — it would pass every on-device" >&2
    echo "  gate and fail only at the install commit. See docs/releasing.md and" >&2
    echo "  android/keystore/README.md." >&2
    return 1
  fi
  [ -f "$CHARTER_SIGNING_LINEAGE_FILE" ] \
    || { echo "FATAL: CHARTER_SIGNING_LINEAGE_FILE=$CHARTER_SIGNING_LINEAGE_FILE not found" >&2; return 1; }
  local apksigner
  apksigner=$(_android_apksigner) || return 1
  echo "re-signing $apk with the release key's v3 rotation lineage…" >&2
  "$apksigner" sign \
    --ks "$CHARTER_KEYSTORE_FILE" --ks-key-alias "$CHARTER_KEY_ALIAS" \
    --ks-pass env:CHARTER_KEYSTORE_PASSWORD --key-pass env:CHARTER_KEY_PASSWORD \
    --lineage "$CHARTER_SIGNING_LINEAGE_FILE" --rotation-min-sdk-version 28 \
    --v1-signing-enabled false --v2-signing-enabled false --v3-signing-enabled true \
    --in "$apk" --out "$apk.rot" \
    || { echo "FATAL: apksigner sign (rotation) failed for $apk" >&2; return 1; }
  mv "$apk.rot" "$apk"
}

# android_verify_signing <apk>
#
# The read of an APK's shipped signing certificate (rotation plan, C2):
# `apksigner verify --print-certs | head -1` was fine when there was only
# ever one signer, ever. A rotated APK carries a lineage, and nothing was
# checking that the rotation is the one this pipeline actually produced.
#
# The v3 + lineage guards below apply ONLY when CHARTER_KEYSTORE_FILE is set
# — i.e. android_sign_rotated actually ran the mandatory rotation above.
# Under the alpha bridge (no CHARTER_KEYSTORE_FILE) the APK is a plain
# gradle-signed build with no lineage at all (verified empirically: AGP's
# debug signingConfig here produces v2 only, v3 false) — that local dev path
# is unchanged by this rotation plan, so it only gets the single-signer read.
#
# With CHARTER_KEYSTORE_FILE set, guards — any failure is FATAL and prints
# apksigner's own output:
#   1. v3 verified true (--rotation-min-sdk-version 28 above is v3-only).
#   2. exactly one CURRENT signer ("Number of signers: 1").
#   3. that signer is NOT the well-known debug cert (DEBUG_CERT_SHA256) —
#      real signing material that resolves to the debug cert is refused
#      outright, never treated as a trivial "no rotation needed" pass.
#   4. if RELEASE_CERT_SHA256 is pinned (non-empty), the signer must equal
#      it exactly.
#   5. the APK's lineage must start AT the debug cert and its last signer
#      must equal the current one — i.e. this is a rotation FROM the
#      fielded cert, not an unrelated key carrying a lineage of its own.
# Prints the current signer's SHA-256 digest (lowercase hex) on success.
android_verify_signing() {
  local apk=$1
  local apksigner
  apksigner=$(_android_apksigner) || return 1

  local verify_out
  verify_out=$("$apksigner" verify -v --print-certs "$apk") \
    || { echo "FATAL: apksigner verify failed for $apk" >&2; return 1; }

  if [ -n "${CHARTER_KEYSTORE_FILE:-}" ]; then
    echo "$verify_out" | grep -q '^Verified using v3 scheme (APK Signature Scheme v3): true$' || {
      echo "FATAL: $apk is not v3-verified (expected: it was signed with android_sign_rotated)" >&2
      echo "$verify_out" >&2
      return 1
    }
  fi
  local signers
  signers=$(echo "$verify_out" | grep -oE '^Number of signers: [0-9]+' | grep -oE '[0-9]+')
  [ "$signers" = "1" ] || {
    echo "FATAL: $apk reports $signers current signers, expected exactly 1" >&2
    echo "$verify_out" >&2
    return 1
  }
  local cert
  cert=$(echo "$verify_out" | grep -oiE '^Signer #1 certificate SHA-256 digest: [0-9a-f]+' \
    | awk '{print tolower($NF)}')
  [ -n "$cert" ] || { echo "FATAL: could not read the signing cert digest from $apk" >&2; return 1; }

  if [ -n "${CHARTER_KEYSTORE_FILE:-}" ] && [ "$cert" = "$DEBUG_CERT_SHA256" ]; then
    echo "FATAL: $apk's current signer IS the debug cert even though real signing material was" >&2
    echo "  supplied (CHARTER_KEYSTORE_FILE is set) — the keystore is (or has become) the debug" >&2
    echo "  key itself. Refusing: a real release must never ship signed with the well-known" >&2
    echo "  debug cert. See docs/releasing.md." >&2
    return 1
  fi
  if [ -n "${CHARTER_KEYSTORE_FILE:-}" ] && [ -n "${RELEASE_CERT_SHA256:-}" ] && [ "$cert" != "$RELEASE_CERT_SHA256" ]; then
    echo "FATAL: $apk's signer $cert does not match the pinned RELEASE_CERT_SHA256 ($RELEASE_CERT_SHA256)" >&2
    return 1
  fi

  if [ -n "${CHARTER_KEYSTORE_FILE:-}" ] && [ "$cert" != "$DEBUG_CERT_SHA256" ]; then
    local lineage_out
    lineage_out=$("$apksigner" lineage --in "$apk" --print-certs) \
      || { echo "FATAL: apksigner lineage failed for $apk" >&2; return 1; }
    echo "$lineage_out" | grep -qi "Signer #1 in lineage certificate SHA-256 digest: $DEBUG_CERT_SHA256" || {
      echo "FATAL: $apk's lineage does not start at the fielded debug cert ($DEBUG_CERT_SHA256)" >&2
      echo "$lineage_out" >&2
      return 1
    }
    echo "$lineage_out" | grep -qi "certificate SHA-256 digest: $cert" || {
      echo "FATAL: $apk's lineage does not end at its own current signer ($cert)" >&2
      echo "$lineage_out" >&2
      return 1
    }
  fi
  echo "$cert"
}
