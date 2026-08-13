#!/usr/bin/env bash
# Build libcharter_guardian_jni.so for both shipping ABIs into
# carrier/src/main/jniLibs. Requires: `source ~/Android/env.sh` + cargo-ndk.
set -euo pipefail
cd "$(dirname "$0")/../jni-guardian"

PROFILE="${1:-release}"
case "$PROFILE" in
  release) cargo ndk -t arm64-v8a -t x86_64 -o ../carrier/src/main/jniLibs build --release ;;
  debug)   cargo ndk -t arm64-v8a -t x86_64 -o ../carrier/src/main/jniLibs build ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

# 16 KB page alignment (Android 15 / GrapheneOS requirement) — same gate as
# the ward .so.
for so in ../carrier/src/main/jniLibs/*/libcharter_guardian_jni.so; do
  align=$("$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf -l "$so" \
    | awk '/LOAD/ {print $NF}' | sort -u | head -1)
  if [ "$align" != "0x4000" ]; then
    echo "FATAL: $so LOAD align $align != 0x4000 (16 KB)" >&2
    exit 1
  fi
done
echo "OK: guardian jniLibs built ($PROFILE), 16 KB aligned."
