#!/bin/bash
# Build the conventional x86-64 Windows GUI baseline from any supported host.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET=x86_64-pc-windows-msvc
TARGET_DIR="$HERE/target"
DIST="$HERE/dist"

command -v cargo-xwin >/dev/null 2>&1 || {
  echo "cargo-xwin is required" >&2
  exit 2
}

mkdir -p "$DIST"
CARGO_TARGET_DIR="$TARGET_DIR" cargo xwin build \
  --manifest-path "$HERE/Cargo.toml" --target "$TARGET" --release --locked
cp "$TARGET_DIR/$TARGET/release/minicon-lab-hello-window.exe" \
  "$DIST/helloworld-x86-64.exe"
shasum -a 256 "$DIST/helloworld-x86-64.exe" | tee \
  "$DIST/helloworld-x86-64.exe.sha256"
file "$DIST/helloworld-x86-64.exe"
