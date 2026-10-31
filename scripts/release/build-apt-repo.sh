#!/usr/bin/env bash
# Build the signed apt repository metadata for the published `kintrinsic` .deb.
#
#   build-apt-repo.sh --deb <path> --url <public-download-url> [--sha256 <64hex>]
#                     [--out site/apt] [--self-check]
#
# Writes <out>/dists/ (Packages, Packages.gz, Release, InRelease, Release.gpg)
# and <out>/pool.json. It never writes the .deb into the repo (the deploy
# fetches it from the GitHub release and verifies the hash: see
# fetch-apt-pool.sh), and never touches the public keyring files in <out>.
#
# Signing uses the dedicated archive key in a separate GNUPGHOME (see
# docs/releasing.md, "The apt repository"):
#   CHARTER_APT_GNUPGHOME  default ~/.charter-release/gnupg
#   CHARTER_APT_KEY_FPR    default F38D996309508C434BD4A2F68535F6586E504E80
#
# Needs: gpg, gpgv, dpkg-deb, dpkg-scanpackages, apt-ftparchive, gzip, sha256sum
# (and apt-get / apt-cache for --self-check).
set -euo pipefail

die() { echo "build-apt-repo: error: $*" >&2; exit 1; }
log() { echo "build-apt-repo: $*"; }

usage() {
  sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-2}"
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

DEB=""
URL=""
WANT_SHA=""
OUT="$REPO_ROOT/site/apt"
SELF_CHECK=0

while [ $# -gt 0 ]; do
  case "$1" in
    --deb)        [ $# -ge 2 ] || die "--deb needs a value"; DEB="$2"; shift 2 ;;
    --url)        [ $# -ge 2 ] || die "--url needs a value"; URL="$2"; shift 2 ;;
    --sha256)     [ $# -ge 2 ] || die "--sha256 needs a value"; WANT_SHA="$2"; shift 2 ;;
    --out)        [ $# -ge 2 ] || die "--out needs a value"; OUT="$2"; shift 2 ;;
    --self-check) SELF_CHECK=1; shift ;;
    -h|--help)    usage 0 ;;
    *)            echo "build-apt-repo: unknown argument: $1" >&2; usage 2 ;;
  esac
done

[ -n "$DEB" ] || { echo "build-apt-repo: --deb is required" >&2; usage 2; }
[ -n "$URL" ] || { echo "build-apt-repo: --url is required" >&2; usage 2; }
[ -f "$DEB" ] || die "no such .deb: $DEB"
case "$URL" in
  https://*) ;;
  *) die "--url must be an https:// URL (got: $URL)" ;;
esac
# pool.json is written by hand below, so keep the URL free of JSON metacharacters.
if printf '%s' "$URL" | grep -q '[^A-Za-z0-9._~:/?#@!$&()*+,;=%-]'; then
  die "--url contains characters that are not safe to embed: $URL"
fi

for tool in gpg gpgv dpkg-deb dpkg-scanpackages apt-ftparchive gzip sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool"
done
if [ "$SELF_CHECK" = 1 ]; then
  for tool in apt-get apt-cache; do
    command -v "$tool" >/dev/null 2>&1 || die "--self-check needs $tool"
  done
fi

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
KEYRING="$OUT/kintrinsic-archive-keyring.gpg"
[ -s "$KEYRING" ] || die "public keyring missing: $KEYRING (the build never creates it)"

GNUPGHOME="${CHARTER_APT_GNUPGHOME:-$HOME/.charter-release/gnupg}"
KEY_FPR="${CHARTER_APT_KEY_FPR:-F38D996309508C434BD4A2F68535F6586E504E80}"
[ -d "$GNUPGHOME" ] || die "signing GNUPGHOME not found: $GNUPGHOME (set CHARTER_APT_GNUPGHOME)"
export GNUPGHOME
export LC_ALL=C

# ---- verify the input -------------------------------------------------------
ACTUAL_SHA="$(sha256sum "$DEB" | cut -d' ' -f1)"
if [ -n "$WANT_SHA" ]; then
  printf '%s' "$WANT_SHA" | grep -Eq '^[0-9a-fA-F]{64}$' || die "--sha256 is not 64 hex characters"
  WANT_SHA="$(printf '%s' "$WANT_SHA" | tr 'A-F' 'a-f')"
  [ "$ACTUAL_SHA" = "$WANT_SHA" ] || die "sha256 mismatch for $DEB: expected $WANT_SHA, got $ACTUAL_SHA"
fi
SIZE="$(stat -c %s "$DEB")"
PKG="$(dpkg-deb -f "$DEB" Package)"
VER="$(dpkg-deb -f "$DEB" Version)"
ARCH="$(dpkg-deb -f "$DEB" Architecture)"
[ "$PKG" = "kintrinsic" ] || die "unexpected Package '$PKG' (want kintrinsic)"
[ "$ARCH" = "amd64" ] || die "unexpected Architecture '$ARCH' (the repository is amd64 only)"
printf '%s' "$VER" | grep -Eq '^[0-9][A-Za-z0-9.+~-]*$' || die "unexpected Version '$VER'"
NAME="${PKG}_${VER}_${ARCH}.deb"
POOL_REL="pool/main/k/kintrinsic/$NAME"
log "package $PKG $VER ($ARCH), $SIZE bytes, sha256 $ACTUAL_SHA"

# ---- scratch space ----------------------------------------------------------
TMP="$(mktemp -d "${TMPDIR:-/tmp}/kintrinsic-apt.XXXXXX")"
NEW_DISTS="" ; OLD_DISTS="" ; NEW_JSON="" ; SWAPPED=0 ; DONE=0
cleanup() {
  rm -rf "$TMP"
  [ -z "$NEW_DISTS" ] || rm -rf "$NEW_DISTS"
  [ -z "$NEW_JSON" ] || rm -f "$NEW_JSON"
  if [ "$DONE" = 0 ] && [ "$SWAPPED" = 1 ]; then
    # The swap started but did not finish: put the previous dists/ back.
    rm -rf "$OUT/dists"
    if [ -e "$OLD_DISTS" ]; then mv "$OLD_DISTS" "$OUT/dists"; fi
  elif [ -n "$OLD_DISTS" ]; then
    rm -rf "$OLD_DISTS"
  fi
}
trap cleanup EXIT

TREE="$TMP/tree"
mkdir -p "$TREE/pool/main/k/kintrinsic" "$TREE/dists/stable/main/binary-amd64"
cp "$DEB" "$TREE/$POOL_REL"

# ---- Packages ---------------------------------------------------------------
BIN="$TREE/dists/stable/main/binary-amd64"
( cd "$TREE" && dpkg-scanpackages --arch amd64 pool /dev/null ) >"$BIN/Packages" 2>"$TMP/scan.err" \
  || { cat "$TMP/scan.err" >&2; die "dpkg-scanpackages failed"; }
[ "$(grep -c '^Package: ' "$BIN/Packages")" = 1 ] || die "Packages must hold exactly one entry"
grep -qx "Filename: $POOL_REL" "$BIN/Packages" || die "Packages Filename is not $POOL_REL"
grep -qx "SHA256: $ACTUAL_SHA" "$BIN/Packages" || die "Packages SHA256 does not match the .deb"
gzip -9n -c "$BIN/Packages" >"$BIN/Packages.gz"
cat >"$BIN/Release" <<REL
Archive: stable
Origin: Kintrinsic
Label: Kintrinsic
Component: main
Architecture: amd64
REL

# ---- Release ----------------------------------------------------------------
# apt-ftparchive hashes every file under the directory it is pointed at, so the
# result is written outside the tree and only moved in once it is signed.
(
  cd "$TREE/dists/stable"
  apt-ftparchive \
    -o APT::FTPArchive::Release::Origin=Kintrinsic \
    -o APT::FTPArchive::Release::Label=Kintrinsic \
    -o APT::FTPArchive::Release::Suite=stable \
    -o APT::FTPArchive::Release::Codename=stable \
    -o APT::FTPArchive::Release::Architectures=amd64 \
    -o APT::FTPArchive::Release::Components=main \
    -o "APT::FTPArchive::Release::Description=Kintrinsic Linux warden (charterd) — official releases" \
    -o APT::FTPArchive::MD5=true \
    -o APT::FTPArchive::SHA1=false \
    -o APT::FTPArchive::SHA256=true \
    -o APT::FTPArchive::SHA512=true \
    release .
) >"$TMP/Release" 2>"$TMP/release.err" || { cat "$TMP/release.err" >&2; die "apt-ftparchive release failed"; }
for f in Packages Packages.gz Release; do
  grep -q " main/binary-amd64/$f\$" "$TMP/Release" || die "Release does not list main/binary-amd64/$f"
done
for sect in MD5Sum SHA256 SHA512; do
  grep -qx "$sect:" "$TMP/Release" || die "Release is missing its $sect section"
done
if grep -Eq '^ [0-9a-f]+ +[0-9]+ +(Release|InRelease|Release\.gpg)$' "$TMP/Release"; then
  die "Release must not hash Release/InRelease/Release.gpg"
fi
grep -q '^Valid-Until:' "$TMP/Release" && die "Release must not carry Valid-Until"
cp "$TMP/Release" "$TREE/dists/stable/Release"

# ---- sign -------------------------------------------------------------------
gpg_sign() {
  gpg --batch --yes --pinentry-mode loopback --passphrase '' \
    --local-user "$KEY_FPR" --digest-algo SHA512 "$@"
}
gpg --batch --list-secret-keys "$KEY_FPR" >/dev/null 2>&1 \
  || die "signing key $KEY_FPR not found in GNUPGHOME=$GNUPGHOME"
gpg_sign --clearsign -o "$TREE/dists/stable/InRelease" "$TREE/dists/stable/Release" \
  || die "gpg --clearsign failed"
gpg_sign --armor --detach-sign -o "$TREE/dists/stable/Release.gpg" "$TREE/dists/stable/Release" \
  || die "gpg detached signing failed"

# Verify against the committed public keyring BEFORE anything is replaced.
gpgv --keyring "$KEYRING" "$TREE/dists/stable/InRelease" 2>"$TMP/gpgv.err" \
  || { cat "$TMP/gpgv.err" >&2; die "InRelease does not verify against $KEYRING"; }
gpgv --keyring "$KEYRING" "$TREE/dists/stable/Release.gpg" "$TREE/dists/stable/Release" 2>"$TMP/gpgv2.err" \
  || { cat "$TMP/gpgv2.err" >&2; die "Release.gpg does not verify against $KEYRING"; }

# ---- pool.json --------------------------------------------------------------
printf '{\n  "pool": [\n    {\n      "filename": "%s",\n      "url": "%s",\n      "sha256": "%s",\n      "sizeBytes": %s\n    }\n  ]\n}\n' \
  "$POOL_REL" "$URL" "$ACTUAL_SHA" "$SIZE" >"$TMP/pool.json"

# ---- self-check (on the staged tree, BEFORE <out> is touched) ----------------
if [ "$SELF_CHECK" = 1 ]; then
  echo "== self-check: gpgv =="
  gpgv --keyring "$KEYRING" "$TREE/dists/stable/InRelease" || die "self-check: gpgv failed"

  echo "== self-check: apt-get update against a file: copy =="
  CHK="$TMP/check"
  mkdir -p "$CHK/state/lists/partial" "$CHK/cache/archives/partial"
  # $TREE already holds dists/ and the pool file; apt only reads it.
  cp -R "$TREE" "$CHK/repo"
  cp "$KEYRING" "$CHK/keyring.gpg"
  : >"$CHK/state/status"
  echo "deb [signed-by=$CHK/keyring.gpg arch=amd64] file:$CHK/repo stable main" >"$CHK/sources.list"
  APT_OPTS=(
    -o "Dir::State=$CHK/state"
    -o "Dir::State::status=$CHK/state/status"
    -o "Dir::Cache=$CHK/cache"
    -o "Dir::Etc::sourcelist=$CHK/sources.list"
    -o "Dir::Etc::sourceparts=-"
    -o "Dir::Etc::trusted=$CHK/keyring.gpg"
    -o "Dir::Etc::trustedparts=-"
    -o "APT::Architecture=amd64"
  )
  apt-get "${APT_OPTS[@]}" update || die "self-check: apt-get update failed"
  echo "== self-check: apt-cache policy kintrinsic =="
  POLICY="$(apt-cache "${APT_OPTS[@]}" policy kintrinsic)" || die "self-check: apt-cache policy failed"
  printf '%s\n' "$POLICY"
  printf '%s\n' "$POLICY" | grep -Eq "^  Candidate: $(printf '%s' "$VER" | sed 's/[.+~]/\\&/g')\$" \
    || die "self-check: apt candidate is not $VER"
  echo "== self-check passed =="
fi

# ---- install atomically (same filesystem as <out>) ---------------------------
NEW_DISTS="$OUT/.dists.new.$$"
OLD_DISTS="$OUT/.dists.old.$$"
NEW_JSON="$OUT/.pool.json.new.$$"
cp -R "$TREE/dists" "$NEW_DISTS"
cp "$TMP/pool.json" "$NEW_JSON"
if [ -e "$OUT/dists" ]; then mv "$OUT/dists" "$OLD_DISTS"; fi
SWAPPED=1
mv "$NEW_DISTS" "$OUT/dists"; NEW_DISTS=""
mv "$NEW_JSON" "$OUT/pool.json"; NEW_JSON=""
DONE=1
log "wrote $OUT/dists and $OUT/pool.json"

log "done: kintrinsic $VER"
