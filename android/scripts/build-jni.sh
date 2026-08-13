#!/usr/bin/env bash
# Build libcharter_jni.so for both shipping ABIs into app/src/main/jniLibs.
# Release carries real-relay ONLY (never `mock`); debug adds the test entry
# points. Requires: `source ~/Android/env.sh` (SDK/NDK) + cargo-ndk.
set -euo pipefail
cd "$(dirname "$0")/../jni"

PROFILE="${1:-release}"
OUT=../app/src/main/jniLibs

# Start from an empty staging dir. cargo-ndk does NOT overwrite a destination
# that is newer than what it just built, so a debug build followed by a release
# publish left the DEBUG library — mock test seams and all — staged for
# packaging, while the "no mock" gate below still passed because it inspects
# the dependency graph rather than the file that actually ships. That published
# a 140 MB APK carrying mock entry points on 2026-07-28; it was caught by the
# size, which is not a control we should rely on.
rm -rf "$OUT"

case "$PROFILE" in
  release)
    cargo ndk -t arm64-v8a -t x86_64 -o "$OUT" \
      build --release --no-default-features --features real-relay
    # Gate: the shipped graph must carry no `mock`.
    if cargo tree -e features --no-default-features --features real-relay | grep -q ' mock'; then
      echo "FATAL: mock feature leaked into the release graph" >&2
      exit 1
    fi
    ;;
  debug)
    cargo ndk -t arm64-v8a -t x86_64 -o "$OUT" \
      build --no-default-features --features real-relay,mock
    ;;
  *)
    echo "usage: $0 [release|debug]" >&2
    exit 2
    ;;
esac

# The staged libraries must BE the ones just built — not a survivor of an
# earlier build with different features. Byte-compare against the artifacts
# cargo produced this run.
case "$PROFILE" in
  release) BUILT_DIR=release ;;
  debug) BUILT_DIR=debug ;;
esac
for pair in "arm64-v8a:aarch64-linux-android" "x86_64:x86_64-linux-android"; do
  abi=${pair%%:*}
  triple=${pair##*:}
  built="target/$triple/$BUILT_DIR/libcharter_jni.so"
  staged="$OUT/$abi/libcharter_jni.so"
  if ! cmp -s "$built" "$staged"; then
    echo "FATAL: $staged is not the $PROFILE library just built ($built)" >&2
    exit 1
  fi
done

# 16 KB page alignment (Android 15 / GrapheneOS requirement).
for so in "$OUT"/*/libcharter_jni.so; do
  align=$("$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf -l "$so" \
    | awk '/LOAD/ {print $NF}' | sort -u | head -1)
  if [ "$align" != "0x4000" ]; then
    echo "FATAL: $so LOAD align $align != 0x4000 (16 KB)" >&2
    exit 1
  fi
done
echo "OK: jniLibs built ($PROFILE), 16 KB aligned, no mock in release."
