#!/usr/bin/env sh
# Builds the Rust core for Android and generates its Kotlin bindings.
#
# Needs: the Android NDK (ANDROID_NDK_HOME), cargo-ndk, the Rust targets
# aarch64-linux-android and x86_64-linux-android, plus the usual Tether build
# tools (CMake, Python 3 with protobuf, protoc, libclang) and perl + make for
# OpenSSL. Run on Linux (or macOS); see docs/building.md.
set -eu
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-release}"
APP=android/app/src/main

# libtether_ffi.so for each ABI into jniLibs/<abi>/.
cargo ndk -t arm64-v8a -t x86_64 -P 30 -o "$APP/jniLibs" \
    build --profile "$PROFILE" -p tether-ffi

# Kotlin bindings, read from the built library's UniFFI metadata.
case "$PROFILE" in dev) DIR=debug ;; *) DIR="$PROFILE" ;; esac
cargo run -q -p uniffi-bindgen -- generate \
    --library "target/aarch64-linux-android/$DIR/libtether_ffi.so" \
    --language kotlin --no-format --out-dir "$APP/java"
