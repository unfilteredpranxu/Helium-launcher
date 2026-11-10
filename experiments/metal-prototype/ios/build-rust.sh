#!/bin/bash
set -euo pipefail
: "${SRCROOT:?Run this script through the standalone Xcode project}"
if [ "${PLATFORM_NAME:-}" != iphoneos ] || [ "${ARCHS:-}" != arm64 ]; then
    echo 'error: Metal Probe supports physical arm64 iOS devices only, not Simulator.' >&2
    exit 1
fi
export PATH="$HOME/.cargo/bin:$PATH"
command -v rustup >/dev/null || { echo 'error: Install Rust 1.95.0 with rustup first.' >&2; exit 1; }
# `rustup run` fails if missing instead of downloading a toolchain during the build.
rustup run 1.95.0 rustc --version
probe_sysroot=$(rustup run 1.95.0 rustc --print sysroot)
test -d "$probe_sysroot/lib/rustlib/aarch64-apple-ios/lib" || {
    echo 'error: Run rustup target add --toolchain 1.95.0 aarch64-apple-ios first.' >&2
    exit 1
}
export CARGO_TARGET_DIR="$SRCROOT/../target"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
rustup run 1.95.0 cargo build --locked --release --lib \
    --target aarch64-apple-ios --manifest-path "$SRCROOT/../Cargo.toml"
