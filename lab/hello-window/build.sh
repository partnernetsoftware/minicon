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
build_variant() {
  local output="$1"
  shift
  (
  cd "$HERE"
  CARGO_TARGET_DIR="$TARGET_DIR" RUSTFLAGS="-C link-arg=/Brepro" cargo xwin build \
      --manifest-path Cargo.toml --target "$TARGET" --release --locked "$@"
  )
  cp "$TARGET_DIR/$TARGET/release/minicon-lab-hello-window.exe" "$DIST/$output"
}

build_variant helloworld-x86-64.exe --no-default-features
build_variant helloworld-resourced-x86-64.exe --no-default-features --features resourced

for output in helloworld-x86-64.exe helloworld-resourced-x86-64.exe; do
  if objdump -x "$DIST/$output" 2>/dev/null | \
    grep -Eiq 'DLL Name:.*(VCRUNTIME|MSVCP|MSVCR)[0-9._-]*\.dll'; then
    echo "$output unexpectedly imports the Visual C++ Redistributable" >&2
    exit 1
  fi
  shasum -a 256 "$DIST/$output" | tee "$DIST/$output.sha256"
  file "$DIST/$output"
done

if ! objdump -x "$DIST/helloworld-resourced-x86-64.exe" 2>/dev/null | \
  grep -q 'Resource Directory \[.rsrc\]'; then
  echo "resourced comparison has no PE Resource Directory" >&2
  exit 1
fi
