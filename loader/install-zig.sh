#!/bin/bash
# SHA-pinned Zig 0.16.0 for the macos-15 (aarch64) pack job. Not Homebrew.
# Always restore PREFIX from the verified tar; an existing zig binary is not a pin.
set -euo pipefail
VER="${ZIG_VERSION:-0.16.0}"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64|aarch64)
    URL="https://ziglang.org/download/${VER}/zig-aarch64-macos-${VER}.tar.xz"
    EXPECT="${ZIG_SHA256:-b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489}"
    PREFIX="zig-aarch64-macos-${VER}"
    ;;
  x86_64)
    URL="https://ziglang.org/download/${VER}/zig-x86_64-macos-${VER}.tar.xz"
    EXPECT="${ZIG_SHA256:-0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7}"
    PREFIX="zig-x86_64-macos-${VER}"
    ;;
  *)
    echo "unsupported uname -m $ARCH" >&2
    exit 2
    ;;
esac
DEST="${ZIG_DIR:-$HOME/zig-${VER}}"
TAR="$DEST/${PREFIX}.tar.xz"
mkdir -p "$DEST"
if [[ ! -s "$TAR" ]] || [[ "$(shasum -a 256 "$TAR" | awk '{print $1}')" != "$EXPECT" ]]; then
  curl --http1.1 -fL --retry 5 --retry-delay 3 -C - -o "$TAR" "$URL"
fi
got=$(shasum -a 256 "$TAR" | awk '{print $1}')
if [[ "$got" != "$EXPECT" ]]; then
  echo "[zig] SHA256 mismatch: got $got want $EXPECT" >&2
  exit 1
fi

STAGE=$(mktemp -d "${TMPDIR:-/tmp}/zig-stage.XXXXXX")
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT
tar -xJf "$TAR" -C "$STAGE"
test -x "$STAGE/$PREFIX/zig"
test "$("$STAGE/$PREFIX/zig" version)" = "$VER"

LIVE="$DEST/$PREFIX"
BAK="${LIVE}.bak.$$"
if [[ -e "$LIVE" ]]; then
  mv "$LIVE" "$BAK"
fi
if ! mv "$STAGE/$PREFIX" "$LIVE"; then
  if [[ -e "$BAK" ]]; then mv "$BAK" "$LIVE"; fi
  echo "[zig] atomic replace failed" >&2
  exit 1
fi
rm -rf "$BAK"
rmdir "$STAGE" 2>/dev/null || true
trap - EXIT

test -x "$LIVE/zig"
test "$("$LIVE/zig" version)" = "$VER"
echo "$LIVE"
