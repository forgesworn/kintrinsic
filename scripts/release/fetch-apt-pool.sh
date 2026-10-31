#!/usr/bin/env bash
# Materialise the apt pool (the .deb files) from <apt-dir>/pool.json.
#
#   fetch-apt-pool.sh [<apt-dir>=site/apt]
#
# Before anything is downloaded the repository index is verified end to end:
#   1. InRelease must verify (gpgv) against <apt-dir>/kintrinsic-archive-keyring.gpg;
#   2. the sha256 of dists/stable/main/binary-amd64/Packages and Packages.gz
#      must equal the SHA256 entries for them inside that signed InRelease
#      (a file that is not listed there is a failure).
# Each pool entry is then downloaded from its GitHub release URL, hashed, and
# kept only when its sha256 matches pool.json AND the SHA256 that the verified
# Packages index records for that Filename. Any mismatch deletes the file and
# exits non-zero: the deploy fails closed and the previous site stays live.
# Files already present with the right hash are skipped.
set -euo pipefail

die() { echo "fetch-apt-pool: error: $*" >&2; exit 1; }

case "${1:-}" in -h|--help) sed -n '2,14p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;; esac
[ $# -le 1 ] || die "usage: fetch-apt-pool.sh [<apt-dir>]"
APT_DIR="${1:-site/apt}"
MANIFEST="$APT_DIR/pool.json"
PACKAGES="$APT_DIR/dists/stable/main/binary-amd64/Packages"
[ -f "$MANIFEST" ] || die "missing $MANIFEST"
[ -f "$PACKAGES" ] || die "missing $PACKAGES"
for tool in curl sha256sum awk gpgv; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done

# ---- verify the signed index before trusting anything it says ---------------
KEYRING="$APT_DIR/kintrinsic-archive-keyring.gpg"
INRELEASE="$APT_DIR/dists/stable/InRelease"
[ -s "$KEYRING" ] || die "missing $KEYRING"
[ -f "$INRELEASE" ] || die "missing $INRELEASE"
gpgv --keyring "$KEYRING" "$INRELEASE" || die "InRelease does not verify against $KEYRING"
for rel in main/binary-amd64/Packages main/binary-amd64/Packages.gz; do
  file="$APT_DIR/dists/stable/$rel"
  [ -f "$file" ] || die "missing $file"
  signed="$(awk -v f="$rel" '
    /^SHA256:[ \t]*$/ { in_sec = 1; next }
    /^[^ \t]/ { in_sec = 0 }
    in_sec && $3 == f { print $1 }
  ' "$INRELEASE")"
  [ -n "$signed" ] || die "InRelease does not list $rel under SHA256"
  [ "$(printf '%s\n' "$signed" | wc -l)" = 1 ] || die "InRelease lists $rel more than once"
  got="$(sha256sum "$file" | cut -d' ' -f1)"
  [ "$got" = "$signed" ] || die "$rel sha256 $got does not match the signed InRelease ($signed)"
done
echo "fetch-apt-pool: index verified (InRelease signature, Packages, Packages.gz)"

# filename<TAB>url<TAB>sha256 per entry.
if command -v jq >/dev/null 2>&1; then
  ENTRIES="$(jq -r '.pool[] | [.filename, .url, .sha256] | @tsv' "$MANIFEST")" \
    || die "cannot parse $MANIFEST"
else
  ENTRIES="$(python3 -I -c '
import json, sys
for e in json.load(open(sys.argv[1]))["pool"]:
    print("\t".join((e["filename"], e["url"], e["sha256"])))
' "$MANIFEST")" || die "cannot parse $MANIFEST"
fi
[ -n "$ENTRIES" ] || die "$MANIFEST lists no pool files"

while IFS=$'\t' read -r filename url sha; do
  case "$filename" in
    pool/*) ;;
    *) die "refusing filename outside pool/: $filename" ;;
  esac
  case "$filename" in *..*|*//*) die "refusing suspicious filename: $filename" ;; esac
  printf '%s' "$filename" | grep -Eq '^[A-Za-z0-9._+/~-]+$' || die "unsafe filename: $filename"
  case "$url" in
    https://*) ;;
    *) die "refusing non-https url for $filename: $url" ;;
  esac
  printf '%s' "$sha" | grep -Eq '^[0-9a-f]{64}$' || die "sha256 for $filename is not 64 lowercase hex"

  # The signed index must agree with the manifest.
  indexed="$(awk -v f="$filename" '
    /^$/ { if (fn == f) { print sh; found = 1 }; fn = ""; sh = ""; next }
    /^Filename: / { fn = substr($0, 11) }
    /^SHA256: / { sh = substr($0, 9) }
    END { if (!found && fn == f) print sh }
  ' "$PACKAGES")"
  [ "$indexed" = "$sha" ] || die "Packages does not record sha256 $sha for $filename (got: ${indexed:-nothing})"

  dest="$APT_DIR/$filename"
  if [ -f "$dest" ] && [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$sha" ]; then
    echo "fetch-apt-pool: skip $filename (already present, sha256 ok)"
    continue
  fi

  mkdir -p "$(dirname "$dest")"
  part="$dest.part"
  rm -f "$dest" "$part"
  echo "fetch-apt-pool: fetch $filename <- $url"
  curl -fsSL --retry 3 -o "$part" "$url" || { rm -f "$part"; die "download failed: $url"; }
  got="$(sha256sum "$part" | cut -d' ' -f1)"
  if [ "$got" != "$sha" ]; then
    rm -f "$part"
    die "sha256 mismatch for $filename: expected $sha, got $got"
  fi
  mv "$part" "$dest"
  echo "fetch-apt-pool: ok $filename (sha256 verified)"
done <<<"$ENTRIES"
