#!/usr/bin/env bash
# Lock-parity guard (port-spec §1.4): the shared core is the substrate both the
# Linux warden and the Android .so build against, so every version core resolves
# for a shared crate MUST also appear in linux's lock — otherwise the "same" core
# compiles against different dependency code on the two substrates.
#
# Rule: core's version set for each shared package must be a SUBSET of linux's.
# linux may resolve EXTRA versions (its Linux-only deps — zbus, x11rb, tauri —
# legitimately pull versions core never sees); that is not skew. A version that
# core has and linux lacks IS skew and fails.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

versions() {
    # "name version" pairs, one per line, sorted+deduped.
    awk '
        $1 == "name" { name = $3; gsub(/"/, "", name) }
        $1 == "version" && name != "" { v = $3; gsub(/"/, "", v); print name, v; name = "" }
    ' "$1" | sort -u
}

core_v="$(versions "$root/core/Cargo.lock")"
linux_v="$(versions "$root/linux/Cargo.lock")"

status=0
checked=0
while read -r pkg ver; do
    [ -n "$pkg" ] || continue
    checked=$((checked + 1))
    if ! echo "$linux_v" | grep -qxF "$pkg $ver"; then
        # Only skew if linux builds this package at all (a core-only crate is fine).
        if echo "$linux_v" | awk '{print $1}' | grep -qxF "$pkg"; then
            lv="$(echo "$linux_v" | awk -v p="$pkg" '$1 == p {print $2}' | paste -sd, -)"
            echo "LOCK SKEW: $pkg core=$ver not in linux={$lv}" >&2
            status=1
        fi
    fi
done <<< "$core_v"

if [ "$status" -eq 0 ]; then
    echo "lock parity OK (core ⊆ linux; $checked core packages checked)"
fi
exit "$status"
