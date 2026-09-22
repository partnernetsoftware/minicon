#!/bin/bash
# Cross-link tinygui the same way MiniCon six-cell payloads are linked.
# Darwin: release-fast. Linux: release (LTO). Windows: windows-release.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
DIST="$HERE/dist"
cd "$HERE"
export AGENTERM_NO_ACTIVATE=1

rustup target add \
  aarch64-apple-darwin x86_64-apple-darwin \
  aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu \
  aarch64-pc-windows-msvc x86_64-pc-windows-msvc

if ! command -v llvm-rc >/dev/null; then
  llvm_bin="$(brew --prefix llvm 2>/dev/null)/bin"
  if [[ -x "$llvm_bin/llvm-rc" ]]; then
    export PATH="$llvm_bin:$PATH"
  fi
fi

for fetch_target in \
  aarch64-apple-darwin x86_64-apple-darwin \
  aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu \
  aarch64-pc-windows-msvc x86_64-pc-windows-msvc
do
  cargo fetch --locked --target "$fetch_target"
done

build_one() {
  local cell="$1" target="$2" kind="$3"
  local dir="$HERE/target-six/$cell"
  mkdir -p "$dir" "$DIST/$cell"
  echo "[tinygui] $cell $target ($kind)"
  case "$kind" in
    native-fast)
      CARGO_TARGET_DIR="$dir" cargo build --locked --profile release-fast \
        --bin tinygui --target "$target"
      cp "$dir/$target/release-fast/tinygui" "$DIST/$cell/tinygui"
      ;;
    zig)
      CARGO_TARGET_DIR="$dir" cargo zigbuild --locked --profile release \
        --bin tinygui --target "$target"
      cp "$dir/$target/release/tinygui" "$DIST/$cell/tinygui"
      ;;
    xwin)
      RUSTFLAGS="--remap-path-prefix=${HOME}=~" \
        CARGO_TARGET_DIR="$dir" cargo xwin build --locked \
        --profile windows-release --bin tinygui --target "$target"
      cp "$dir/$target/windows-release/tinygui.exe" "$DIST/$cell/tinygui.exe"
      ;;
    *) echo "unknown kind $kind" >&2; return 2 ;;
  esac
  if [[ "$kind" = xwin ]]; then
    shasum -a 256 "$DIST/$cell/tinygui.exe" | awk '{print $1}' >"$DIST/$cell/tinygui.exe.sha256"
  else
    shasum -a 256 "$DIST/$cell/tinygui" | awk '{print $1}' >"$DIST/$cell/tinygui.sha256"
  fi
}

fail=0
build_one osx-aarch64 aarch64-apple-darwin native-fast &
p1=$!
build_one osx-x86_64 x86_64-apple-darwin native-fast &
p2=$!
build_one lnx-aarch64 aarch64-unknown-linux-gnu zig &
p3=$!
wait $p1 || fail=1
wait $p2 || fail=1
wait $p3 || fail=1
build_one lnx-x86_64 x86_64-unknown-linux-gnu zig || fail=1
build_one win-x86_64 x86_64-pc-windows-msvc xwin || fail=1
build_one win-aarch64 aarch64-pc-windows-msvc xwin || fail=1
[[ "$fail" -eq 0 ]] || exit 1
echo "[tinygui] artifacts"
find "$DIST" -type f \( -name tinygui -o -name 'tinygui.exe' -o -name '*.sha256' \) | sort
