#!/bin/bash
# Host-native MiniCon-shaped tinygui (osx-aarch64 release-fast). Six-cell is build-six.sh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$HERE/dist"
mkdir -p "$DIST/osx-aarch64"
(
  cd "$HERE"
  CARGO_TARGET_DIR="$HERE/target" cargo build --locked --profile release-fast --bin tinygui
)
cp "$HERE/target/release-fast/tinygui" "$DIST/osx-aarch64/tinygui"
shasum -a 256 "$DIST/osx-aarch64/tinygui" | awk '{print $1}' >"$DIST/osx-aarch64/tinygui.sha256"
ls -l "$DIST/osx-aarch64/tinygui"
