#!/bin/bash
# Rebuild six release-fast minicon bins from this tree (Cargo.toml version).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${PAYLOAD_BUILD:-$ROOT/research/minicon-com-loader/payload-build}"
cd "$ROOT"
export AGENTERM_NO_ACTIVATE=1
mkdir -p "$OUT"
rustup target add \
  aarch64-apple-darwin x86_64-apple-darwin \
  aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu \
  aarch64-pc-windows-msvc x86_64-pc-windows-msvc

# A cold shared Cargo Git cache cannot safely have four processes create the
# same target-specific checkout at once. A host-only fetch is insufficient:
# Linux/macOS dependency selection can discover another Git checkout only
# after parallel target builds start. Materialize every locked target graph
# serially, then parallelize only target-isolated compilation.
for fetch_target in \
  aarch64-apple-darwin x86_64-apple-darwin \
  aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu \
  aarch64-pc-windows-msvc x86_64-pc-windows-msvc
do
  cargo fetch --locked --target "$fetch_target"
done

build_one() {
  cell="$1"
  target="$2"
  kind="$3" # native|zig|xwin
  dir="$OUT/$cell"
  mkdir -p "$dir"
  echo "[payload] $cell $target ($kind)"
  case "$kind" in
    native)
      CARGO_TARGET_DIR="$dir" cargo build --locked --profile release-fast --bin minicon --target "$target"
      ;;
    zig)
      CARGO_TARGET_DIR="$dir" cargo zigbuild --locked --profile release-fast --bin minicon --target "$target"
      ;;
    xwin)
      CARGO_TARGET_DIR="$dir" cargo xwin build --locked --profile release-fast --bin minicon --target "$target"
      ;;
  esac
}

# winresource compile() shells llvm-rc (unprefixed on unknown Windows triples).
if ! command -v llvm-rc >/dev/null; then
  llvm_bin="$(brew --prefix llvm 2>/dev/null)/bin"
  if [[ -x "$llvm_bin/llvm-rc" ]]; then
    export PATH="$llvm_bin:$PATH"
  else
    echo "llvm-rc not on PATH (winresource icon embed)" >&2
    exit 2
  fi
fi

# Independent target dirs: 4-way parallel, Windows serial (xwin shim race).
build_one osx-aarch64 aarch64-apple-darwin native &
p1=$!
build_one osx-x86_64 x86_64-apple-darwin native &
p2=$!
build_one lnx-aarch64 aarch64-unknown-linux-gnu zig &
p3=$!
build_one lnx-x86_64 x86_64-unknown-linux-gnu zig &
p4=$!
fail=0
wait $p1 || fail=1
wait $p2 || fail=1
wait $p3 || fail=1
wait $p4 || fail=1
build_one win-x86_64 x86_64-pc-windows-msvc xwin || fail=1
build_one win-aarch64 aarch64-pc-windows-msvc xwin || fail=1
[[ "$fail" -eq 0 ]] || exit 1
echo "[payload] OK $OUT"
