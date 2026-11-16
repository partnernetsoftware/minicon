#!/bin/bash
# Pack one Rust launcher artifact per cell: <name>-<cell>.com
#
# Host-aware: the same script runs on the macOS pack host (CI) and on a Windows
# developer machine, choosing the launcher build that host can actually do.
#
#   macOS host : native for osx-*, cargo-zigbuild for lnx-*, cargo-xwin for win-*
#   Windows    : native for win-x86_64, cargo-xwin + lld-link for win-aarch64,
#                rust-lld self-contained musl for lnx-*
#   Linux host : native for lnx-*, cargo-xwin for win-*, osx-* unsupported
#
# Inputs (defaults match pack.sh staging):
#   MINICON_RUST_CELLS     cells dir              (default loader/dist/cells)
#   MINICON_RUST_COM_OUT   output dir             (default loader/dist/rust)
#   MINICON_RUST_ONLY      comma list of cells    (default: all six)
#   MINICON_RUST_NAME      artifact prefix        (default: minicon)
#   MINICON_RUST_LEAF_WIN  cell file inside win-* (default: minicon.exe)
#   MINICON_RUST_LEAF_UNIX cell file elsewhere    (default: minicon)
#   MINICON_RUST_TARGET_DIR shared CARGO_TARGET_DIR root (default: per-cell under com-rs)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COM_RS="$ROOT/loader/com-rs"
CELLS="${MINICON_RUST_CELLS:-$ROOT/loader/dist/cells}"
OUT="${MINICON_RUST_COM_OUT:-$ROOT/loader/dist/rust}"
ONLY="${MINICON_RUST_ONLY:-lnx-x86_64,lnx-aarch64,win-x86_64,win-aarch64,osx-x86_64,osx-aarch64}"
NAME="${MINICON_RUST_NAME:-minicon}"
LEAF_WIN="${MINICON_RUST_LEAF_WIN:-minicon.exe}"
LEAF_UNIX="${MINICON_RUST_LEAF_UNIX:-minicon}"
TARGET_ROOT="${MINICON_RUST_TARGET_DIR:-}"
VERSION="${MINICON_RUST_VERSION:-}"
SOURCE_SHA="${MINICON_RUST_SOURCE_SHA:-}"

# Cells follow loader/dist/cells (the retired pack.sh staged the same layout).
# v0.2.27 removed pack.sh from the live path, so when the directory is absent
# stage it straight from payload-build (rebuild-payloads.sh's output layout).
if [[ ! -d "$CELLS" ]]; then
    PB="${MINICON_PAYLOAD_BUILD:-$ROOT/loader/payload-build}"
    if [[ -d "$PB" ]]; then
        echo "[pack-rust-all] staging cells from $PB"
        for cell in lnx-x86_64 lnx-aarch64 win-x86_64 win-aarch64 osx-x86_64 osx-aarch64; do
            target=""
            leaf="minicon"
            case "$cell" in
                lnx-x86_64) target="x86_64-unknown-linux-gnu" ;;
                lnx-aarch64) target="aarch64-unknown-linux-gnu" ;;
                win-x86_64) target="x86_64-pc-windows-msvc"; leaf="minicon.exe" ;;
                win-aarch64) target="aarch64-pc-windows-msvc"; leaf="minicon.exe" ;;
                osx-x86_64) target="x86_64-apple-darwin" ;;
                osx-aarch64) target="aarch64-apple-darwin" ;;
            esac
            case "$cell" in
                win-*) src="$PB/$cell/$target/windows-release/$leaf" ;;
                osx-*) src="$PB/$cell/$target/release-fast/$leaf" ;;
                *) src="$PB/$cell/$target/release/$leaf" ;;
            esac
            [[ -f "$src" ]] || { echo "missing payload for $cell: $src (run loader/rebuild-payloads.sh)" >&2; exit 2; }
            mkdir -p "$CELLS/$cell"
            cp "$src" "$CELLS/$cell/$leaf"
        done
    else
        echo "missing cells dir: $CELLS and no payload-build at $PB (run loader/rebuild-payloads.sh)" >&2
        exit 2
    fi
fi
mkdir -p "$OUT"

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) FAMILY=windows ;;
    Darwin*) FAMILY=macos ;;
    *) FAMILY=linux ;;
esac

cargo build --release --manifest-path "$COM_RS/Cargo.toml" --bin com-pack
PACK="$COM_RS/target/release/com-pack"
[[ -x "$PACK" ]] || { echo "com-pack not built" >&2; exit 2; }

HOST="$(rustc -vV | sed -n 's/^host: //p')"

# lld-link is the same LLD as rust-lld, selected by argv[0]; a Windows host has
# no ARM64 link.exe unless the VS ARM64 workload is installed. The linker path
# goes into RUSTFLAGS, which cargo resolves against its own cwd — make it
# absolute so a relative cells/out invocation cannot break the ARM64 build.
LLD_LINK="${MINICON_LLD_LINK:-}"
if [[ "$FAMILY" == windows ]]; then
    if [[ -z "$LLD_LINK" ]]; then
        LLD_LINK="$OUT/.tools/lld-link.exe"
    fi
    case "$LLD_LINK" in
        /*|?:/*|[A-Za-z]:\\*) ;;
        *) LLD_LINK="$(cd "$(dirname "$LLD_LINK")" && pwd)/$(basename "$LLD_LINK")" ;;
    esac
    if [[ ! -f "$LLD_LINK" ]]; then
        mkdir -p "$(dirname "$LLD_LINK")"
        cp "$(rustc --print sysroot)/lib/rustlib/$HOST/bin/rust-lld.exe" "$LLD_LINK"
    fi
fi

# cell -> "target:kind", or nothing when this host cannot build that launcher.
cell_build() {
    case "$FAMILY:$1" in
        macos:osx-x86_64) echo "x86_64-apple-darwin:native" ;;
        macos:osx-aarch64) echo "aarch64-apple-darwin:native" ;;
        macos:lnx-x86_64) echo "x86_64-unknown-linux-gnu:zig" ;;
        macos:lnx-aarch64) echo "aarch64-unknown-linux-gnu:zig" ;;
        macos:win-x86_64) echo "x86_64-pc-windows-msvc:xwin" ;;
        macos:win-aarch64) echo "aarch64-pc-windows-msvc:xwin" ;;
        windows:win-x86_64) echo "x86_64-pc-windows-msvc:native" ;;
        windows:win-aarch64) echo "aarch64-pc-windows-msvc:xwin-lld" ;;
        windows:lnx-x86_64) echo "x86_64-unknown-linux-musl:musl" ;;
        windows:lnx-aarch64) echo "aarch64-unknown-linux-musl:musl" ;;
        linux:lnx-x86_64) echo "x86_64-unknown-linux-gnu:native" ;;
        linux:lnx-aarch64) echo "aarch64-unknown-linux-gnu:native" ;;
        linux:win-x86_64) echo "x86_64-pc-windows-msvc:xwin" ;;
        linux:win-aarch64) echo "aarch64-pc-windows-msvc:xwin" ;;
        *) : ;;
    esac
}

leaf_for() {
    case "$1" in
        win-*) echo "$LEAF_WIN" ;;
        *) echo "$LEAF_UNIX" ;;
    esac
}

build_launcher() {
    cell="$1"
    spec="$(cell_build "$cell")"
    if [[ -z "$spec" ]]; then
        echo "[pack-rust-all] skip $cell (no launcher target on this $FAMILY host)"
        return 0
    fi
    target="${spec%%:*}"
    kind="${spec##*:}"
    if [[ "$HOST" == "$target" ]]; then
        kind=native
    fi
    if [[ -n "$TARGET_ROOT" ]]; then
        dir="$TARGET_ROOT/$cell"
    else
        dir="$COM_RS/target-$cell"
    fi
    echo "[pack-rust-all] launcher $cell -> $target ($kind)"
    # MINICON_COM_VERSION stamps the launcher's embedded VERSIONINFO so the
    # signing preflight can assert ProductVersion without reading the payload.
    export MINICON_COM_VERSION="${MINICON_RUST_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)}"
    case "$kind" in
        native)
            CARGO_TARGET_DIR="$dir" cargo build --release --offline \
                --manifest-path "$COM_RS/Cargo.toml" --bin com-trampoline --target "$target"
            ;;
        zig)
            CARGO_TARGET_DIR="$dir" cargo zigbuild --release --offline \
                --manifest-path "$COM_RS/Cargo.toml" --bin com-trampoline --target "$target"
            ;;
        xwin)
            CARGO_TARGET_DIR="$dir" cargo xwin build --release --offline \
                --manifest-path "$COM_RS/Cargo.toml" --bin com-trampoline --target "$target"
            ;;
        musl)
            CARGO_TARGET_DIR="$dir" RUSTFLAGS="-C linker=rust-lld -C link-self-contained=yes" \
                cargo build --release --offline \
                --manifest-path "$COM_RS/Cargo.toml" --bin com-trampoline --target "$target"
            ;;
        xwin-lld)
            CARGO_TARGET_DIR="$dir" RUSTFLAGS="-C linker=$LLD_LINK" \
                cargo xwin build --release --offline \
                --manifest-path "$COM_RS/Cargo.toml" --bin com-trampoline --target "$target"
            ;;
        *)
            echo "unknown build kind: $kind" >&2
            return 2
            ;;
    esac
    launcher=""
    for candidate in "$dir/$target/release/com-trampoline.exe" "$dir/$target/release/com-trampoline"; do
        if [[ -f "$candidate" ]]; then launcher="$candidate"; break; fi
    done
    [[ -n "$launcher" ]] || { echo "launcher missing under $dir/$target/release" >&2; return 3; }
    meta=(--meta "product=$NAME" --meta "cell=$cell")
    [[ -n "$VERSION" ]] && meta+=(--meta "version=$VERSION")
    [[ -n "$SOURCE_SHA" ]] && meta+=(--meta "source_sha=$SOURCE_SHA")
    "$PACK" pack --trampoline "$launcher" --out "$OUT/$NAME-$cell.com" \
        "${meta[@]}" "$cell=$CELLS/$cell/$(leaf_for "$cell")"
}

IFS=',' read -r -a wanted <<<"$ONLY"
fail=0
for cell in "${wanted[@]}"; do
    [[ -z "$cell" ]] && continue
    build_launcher "$cell" || fail=1
done
[[ "$fail" -eq 0 ]] || exit 1
echo "[pack-rust-all] OK $OUT"
