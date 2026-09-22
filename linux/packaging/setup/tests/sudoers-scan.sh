#!/usr/bin/env bash
# Test harness for the sudoers FILE-syntax scan in ../charter-setup.
#
# charter-setup's blanket-sudo check used to reuse a regex written for
# `sudo -l` OUTPUT against sudoers FILE lines, which never matched anything
# real (see the comment above `sudoers_line_is_blanket_grant` in
# charter-setup). This exercises the line-matcher directly against real
# sudoers syntax, plus the file-collection rules (skip dotted/backup names,
# honour @includedir) via the higher-level `sudoers_scan_blanket`.
#
# Run directly: bash linux/packaging/setup/tests/sudoers-scan.sh
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../charter-setup
source "$HERE/../charter-setup"

fails=0
checks=0

# expect_match <who> <line> <description>
expect_match() {
    checks=$((checks + 1))
    if sudoers_line_is_blanket_grant "$1" "$2"; then
        echo "ok   - match:    $3"
    else
        echo "FAIL - expected match, got no match: $3"
        fails=$((fails + 1))
    fi
}

# expect_no_match <who> <line> <description>
expect_no_match() {
    checks=$((checks + 1))
    if sudoers_line_is_blanket_grant "$1" "$2"; then
        echo "FAIL - expected no match, got match: $3"
        fails=$((fails + 1))
    else
        echo "ok   - no match: $3"
    fi
}

echo "== sudoers_line_is_blanket_grant =="
expect_match     "kid" 'kid ALL=(ALL:ALL) ALL'          "user, full runas pair, blanket ALL"
expect_match     "%sudo" '%sudo ALL=(ALL:ALL) ALL'       "group grant, user is in sudo"
expect_match     "kid" 'kid ALL=(ALL) NOPASSWD: ALL'     "NOPASSWD tag before blanket ALL"
expect_match     "kid" "$(printf '\tkid ALL=(ALL) ALL')" "tab-indented"
expect_no_match  "kid" 'kid ALL=(root) /usr/bin/apt'     "single non-ALL command, not blanket"
expect_no_match  "kid" 'kidney ALL=(ALL) ALL'            "username prefix collision (kidney != kid)"
expect_no_match  "kid" '# kid ALL=(ALL) ALL'             "commented-out line"

echo
echo "== sudoers_scan_blanket (file collection: sudoers.d skip rules + @includedir) =="

WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

mkdir -p "$WORK/sudoers.d" "$WORK/other-include-dir"
cat > "$WORK/sudoers" <<EOF
root ALL=(ALL:ALL) ALL
kid  ALL=(root) /usr/bin/apt
@includedir $WORK/other-include-dir
EOF

# A clean sudoers.d with no grant for kid.
cat > "$WORK/sudoers.d/clean" <<EOF
%admin ALL=(ALL) ALL
EOF

# clean_scan(): no hit expected yet.
_sudoers_collect_files "$WORK/sudoers" collected
found=0
for f in "${collected[@]}"; do
    while IFS= read -r line || [ -n "$line" ]; do
        sudoers_line_is_blanket_grant "kid" "$line" && found=1
    done < "$f"
done
checks=$((checks + 1))
if [ "$found" -eq 0 ]; then
    echo "ok   - no false positive across /etc/sudoers + clean sudoers.d/"
else
    echo "FAIL - false positive scanning a clean sudoers.d"
    fails=$((fails + 1))
fi

# A dotted / backup-suffixed file in sudoers.d must be SKIPPED even though it
# grants kid blanket sudo — sudo itself ignores these names.
echo 'kid ALL=(ALL) ALL' > "$WORK/sudoers.d/kid.disabled"
echo 'kid ALL=(ALL) ALL' > "$WORK/sudoers.d/kid~"
_sudoers_collect_files "$WORK/sudoers" collected
checks=$((checks + 1))
skipped_ok=1
for f in "${collected[@]}"; do
    case "$(basename "$f")" in
        kid.disabled|kid~) skipped_ok=0 ;;
    esac
done
if [ "$skipped_ok" -eq 1 ]; then
    echo "ok   - dotted / '~'-suffixed sudoers.d files are skipped"
else
    echo "FAIL - a dotted or '~' sudoers.d file was scanned"
    fails=$((fails + 1))
fi

# A grant living only in the @includedir target must still be found.
echo 'kid ALL=(ALL) ALL' > "$WORK/other-include-dir/50-kid"
_sudoers_collect_files "$WORK/sudoers" collected
found=0
for f in "${collected[@]}"; do
    while IFS= read -r line || [ -n "$line" ]; do
        sudoers_line_is_blanket_grant "kid" "$line" && found=1
    done < "$f"
done
checks=$((checks + 1))
if [ "$found" -eq 1 ]; then
    echo "ok   - a grant inside an @includedir target is found"
else
    echo "FAIL - a grant inside an @includedir target was missed"
    fails=$((fails + 1))
fi

echo
echo "$checks checks, $fails failed"
[ "$fails" -eq 0 ]
