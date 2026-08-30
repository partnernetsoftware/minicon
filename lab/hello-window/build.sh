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
command -v objdump >/dev/null 2>&1 || {
  echo "objdump is required for the no-redistributable import gate" >&2
  exit 2
}

mkdir -p "$DIST"
(
  cd "$HERE"
  CARGO_TARGET_DIR="$TARGET_DIR" cargo xwin build \
    --manifest-path Cargo.toml --target "$TARGET" --release --locked
)
cp "$TARGET_DIR/$TARGET/release/minicon-lab-hello-window.exe" \
  "$DIST/helloworld-x86-64.exe"

if objdump -x "$DIST/helloworld-x86-64.exe" 2>/dev/null | \
  grep -Eiq 'DLL Name:.*(VCRUNTIME|MSVCP|MSVCR)[0-9._-]*\.dll'; then
  echo "unexpected Visual C++ Redistributable dependency" >&2
  exit 1
fi
shasum -a 256 "$DIST/helloworld-x86-64.exe" | tee \
  "$DIST/helloworld-x86-64.exe.sha256"
file "$DIST/helloworld-x86-64.exe"
