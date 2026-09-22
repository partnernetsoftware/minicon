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

# cargo-xwin owns the same pinned MSVC headers/libraries used by the Rust
# controls. Reuse that environment to compile one ordinary pure-C `/MT` GUI;
# this removes Rust while retaining the Windows SDK, linker and static CRT.
eval "$(cargo xwin env --target "$TARGET")"
command -v clang-cl >/dev/null 2>&1 || {
  echo "clang-cl from the cargo-xwin environment is required" >&2
  exit 2
}
(
  cd "$HERE"
  # CL_FLAGS is emitted by cargo-xwin and intentionally expands into its
  # pinned include arguments. Source/output paths stay relative so clang-cl
  # cannot mistake a POSIX leading slash for an MSVC option.
  # shellcheck disable=SC2086
  clang-cl $CL_FLAGS -Wno-msvc-not-found /nologo /W4 /WX /O1 /MT \
    /DUNICODE /D_UNICODE hello-c.c user32.lib kernel32.lib \
    /link /SUBSYSTEM:WINDOWS /ENTRY:wWinMainCRTStartup /Brepro \
    /OUT:dist/helloworld-pure-c-x86-64.exe
)

for output in \
  helloworld-x86-64.exe \
  helloworld-resourced-x86-64.exe \
  helloworld-pure-c-x86-64.exe; do
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

if objdump -x "$DIST/helloworld-pure-c-x86-64.exe" 2>/dev/null | \
  grep -Eq 'Resource Directory \[.rsrc\].*[1-9a-fA-F]'; then
  echo "pure-C comparison unexpectedly has a PE Resource Directory" >&2
  exit 1
fi
