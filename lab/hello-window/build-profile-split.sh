#!/bin/bash
# Build the same MiniCon source with only strip/profile axes changed.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
TARGET=x86_64-pc-windows-msvc
DIST="$HERE/dist"
DEBUG_TARGET="$HERE/target/qvm-debug-strip"
FAST_TARGET="$HERE/target/qvm-release-fast-unstripped"
DEBUG_OPT_TARGET="$HERE/target/qvm-debug-opt-z"
FAST_OPT_TARGET="$HERE/target/qvm-release-fast-opt-0"

command -v cargo-xwin >/dev/null 2>&1 || {
  echo "cargo-xwin is required" >&2
  exit 2
}
command -v objdump >/dev/null 2>&1 || {
  echo "objdump is required for the import gate" >&2
  exit 2
}
mkdir -p "$DIST"
cd "$ROOT"

build_copy() {
  local target_dir="$1" profile="$2" strip="$3" output="$4"
  local profile_dir=debug
  if [[ "$profile" = dev ]]; then
    CARGO_TARGET_DIR="$target_dir" cargo xwin rustc --locked --bin minicon \
      --target "$TARGET" -- -C "strip=$strip" -C link-arg=/Brepro
  else
    profile_dir="$profile"
    CARGO_TARGET_DIR="$target_dir" cargo xwin rustc --locked --bin minicon \
      --target "$TARGET" --profile "$profile" -- -C "strip=$strip" -C link-arg=/Brepro
  fi
  cp "$target_dir/$TARGET/$profile_dir/minicon.exe" "$DIST/$output"
}

build_copy "$DEBUG_TARGET" dev symbols minicon-debug-stripped-x86-64.exe
build_copy "$DEBUG_TARGET" dev none minicon-debug-unstripped-x86-64.exe
build_copy "$FAST_TARGET" release-fast symbols minicon-release-fast-stripped-x86-64.exe
build_copy "$FAST_TARGET" release-fast none minicon-release-fast-unstripped-x86-64.exe

# Cross only the whole-graph optimization level. Cargo profile environment
# overrides preserve each profile's other settings and apply to dependencies,
# unlike a final-crate-only `cargo rustc -C opt-level=...` argument.
CARGO_PROFILE_DEV_OPT_LEVEL=z CARGO_TARGET_DIR="$DEBUG_OPT_TARGET" \
  cargo xwin rustc --locked --bin minicon --target "$TARGET" -- \
  -C strip=none -C link-arg=/Brepro
cp "$DEBUG_OPT_TARGET/$TARGET/debug/minicon.exe" \
  "$DIST/minicon-debug-opt-z-x86-64.exe"

CARGO_PROFILE_RELEASE_FAST_OPT_LEVEL=0 CARGO_TARGET_DIR="$FAST_OPT_TARGET" \
  cargo xwin rustc --locked --bin minicon --target "$TARGET" \
  --profile release-fast -- -C strip=none -C link-arg=/Brepro
cp "$FAST_OPT_TARGET/$TARGET/release-fast/minicon.exe" \
  "$DIST/minicon-release-fast-opt-0-x86-64.exe"

for output in "$DIST"/minicon-*-x86-64.exe; do
  if objdump -x "$output" 2>/dev/null | \
    grep -Eiq 'DLL Name:.*(VCRUNTIME|MSVCP|MSVCR)[0-9._-]*\.dll'; then
    echo "$(basename "$output") unexpectedly imports the Visual C++ Redistributable" >&2
    exit 1
  fi
  stat -f '%N %z' "$output"
  shasum -a 256 "$output"
done
