#!/bin/bash
# Build bx cells for the architectures this host can produce, then pack
# minicon-bx-<cell>.com through the shared Rust launcher lane.
#
#   Windows : native x64, cargo-xwin + lld-link for ARM64, rust-lld musl for Linux
#   macOS   : native darwin; Linux cells via cargo-zigbuild (CI pack host)
#   Linux   : native; Windows cells via cargo-xwin
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BX="$ROOT/bx"
CELLS="${BX_CELLS:-$BX/cells}"
ONLY="${BX_CELLS_ONLY:-win-x86_64,win-aarch64,lnx-x86_64,lnx-aarch64,osx-x86_64,osx-aarch64}"
OUT="${MINICON_RUST_COM_OUT:-$ROOT/loader/dist/rust}"
TARGET_ROOT="${BX_TARGET_DIR:-}"

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) FAMILY=windows ;;
    Darwin*) FAMILY=macos ;;
    *) FAMILY=linux ;;
esac

HOST="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p "$CELLS"
mkdir -p "$OUT"

# No ARM64 link.exe is installed by default; LLD selects its link flavor from
# argv[0], so the same rust-lld binary serves as lld-link.
LLD_LINK="${MINICON_LLD_LINK:-}"
if [[ "$FAMILY" == windows ]]; then
    if [[ -z "$LLD_LINK" ]]; then
        LLD_LINK="$OUT/.tools/lld-link.exe"
    fi
    if [[ ! -f "$LLD_LINK" ]]; then
        mkdir -p "$(dirname "$LLD_LINK")"
        cp "$(rustc --print sysroot)/lib/rustlib/$HOST/bin/rust-lld.exe" "$LLD_LINK"
    fi
fi

cell_target() {
    case "$FAMILY:$1" in
        windows:win-x86_64) echo "x86_64-pc-windows-msvc:native" ;;
        windows:win-aarch64) echo "aarch64-pc-windows-msvc:xwin-lld" ;;
        windows:lnx-x86_64) echo "x86_64-unknown-linux-musl:musl" ;;
        windows:lnx-aarch64) echo "aarch64-unknown-linux-musl:musl" ;;
        macos:osx-x86_64) echo "x86_64-apple-darwin:native" ;;
        macos:osx-aarch64) echo "aarch64-apple-darwin:native" ;;
        macos:lnx-x86_64) echo "x86_64-unknown-linux-gnu:zig" ;;
        macos:lnx-aarch64) echo "aarch64-unknown-linux-gnu:zig" ;;
        macos:win-x86_64) echo "x86_64-pc-windows-msvc:xwin" ;;
        macos:win-aarch64) echo "aarch64-pc-windows-msvc:xwin" ;;
        linux:lnx-x86_64) echo "x86_64-unknown-linux-gnu:native" ;;
        linux:lnx-aarch64) echo "aarch64-unknown-linux-gnu:native" ;;
        *) : ;;
    esac
}

leaf_for() {
    case "$1" in
        win-*) echo "bx.exe" ;;
        *) echo "bx" ;;
    esac
}

build_cell() {
    cell="$1"
    spec="$(cell_target "$cell")"
    if [[ -z "$spec" ]]; then
        echo "[bx-cells] skip $cell (unsupported on this $FAMILY host)"
        return 0
    fi
    target="${spec%%:*}"
    kind="${spec##*:}"
    if [[ "$HOST" == "$target" ]]; then kind=native; fi
    if [[ -n "$TARGET_ROOT" ]]; then dir="$TARGET_ROOT/$cell"; else dir="$BX/target-$cell"; fi
    echo "[bx-cells] $cell -> $target ($kind)"
    case "$kind" in
        native)
            CARGO_TARGET_DIR="$dir" cargo build --release --offline \
                --manifest-path "$BX/Cargo.toml" --target "$target"
            ;;
        zig)
            CARGO_TARGET_DIR="$dir" cargo zigbuild --release --offline \
                --manifest-path "$BX/Cargo.toml" --target "$target"
            ;;
        xwin)
            CARGO_TARGET_DIR="$dir" cargo xwin build --release --offline \
                --manifest-path "$BX/Cargo.toml" --target "$target"
            ;;
        xwin-lld)
            CARGO_TARGET_DIR="$dir" RUSTFLAGS="-C linker=$LLD_LINK" \
                cargo xwin build --release --offline \
                --manifest-path "$BX/Cargo.toml" --target "$target"
            ;;
        musl)
            CARGO_TARGET_DIR="$dir" RUSTFLAGS="-C linker=rust-lld -C link-self-contained=yes" \
                cargo build --release --offline \
                --manifest-path "$BX/Cargo.toml" --target "$target"
            ;;
    esac
    bin="$dir/$target/release/bx"
    [[ -f "$bin.exe" ]] && bin="$bin.exe"
    if [[ -f "$bin" ]]; then
        mkdir -p "$CELLS/$cell"
        cp "$bin" "$CELLS/$cell/$(leaf_for "$cell")"
        echo "[bx-cells] staged $cell"
    else
        echo "[bx-cells] missing output for $cell: $bin" >&2
        return 1
    fi
}

IFS=',' read -r -a wanted <<<"$ONLY"
fail=0
for cell in "${wanted[@]}"; do
    [[ -z "$cell" ]] && continue
    build_cell "$cell" || fail=1
done
[[ "$fail" -eq 0 ]] || exit 1

BX_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$BX/Cargo.toml" | head -n1)"
BX_SHA="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)"

MINICON_RUST_CELLS="$CELLS" \
MINICON_RUST_NAME="minicon-bx" \
MINICON_RUST_LEAF_WIN="bx.exe" \
MINICON_RUST_LEAF_UNIX="bx" \
MINICON_RUST_ONLY="$ONLY" \
MINICON_RUST_VERSION="$BX_VERSION" \
MINICON_RUST_SOURCE_SHA="$BX_SHA" \
MINICON_RUST_COM_OUT="$OUT" \
    bash "$ROOT/loader/pack-rust-all.sh"
