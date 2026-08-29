#!/bin/bash
# Always restore from the SHA-pinned 4.0.2 zip. Existing DEST/bin is not a pin;
# --version is GCC 14.1.0 and does not identify the Cosmopolitan release.
set -euo pipefail
VER="${COSMOCC_VERSION:-4.0.2}"
DEST="${COSMOCC_DIR:-$HOME/cosmocc}"
ZIP="$DEST/cosmocc-${VER}.zip"
EXPECT_SHA="${COSMOCC_SHA256:-85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44}"
EXPECT_BIN="${COSMOCC_BIN_SHA256:-eef9db8fabfc0c08f1930cbba87f60f69a1c49f28e4de006a1b0c6863e943e4b}"
URL1="https://github.com/jart/cosmopolitan/releases/download/${VER}/cosmocc-${VER}.zip"
URL2="https://cosmo.zip/pub/cosmocc/cosmocc-${VER}.zip"
mkdir -p "$DEST"

sha_of() { shasum -a 256 "$1" | awk '{print $1}'; }

ensure_zip() {
  if [[ ! -s "$ZIP" ]]; then
    echo "[cosmocc] download $VER"
    curl --http1.1 -fL --retry 5 --retry-delay 3 -C - -o "$ZIP" "$URL1" || \
      curl --http1.1 -fL --retry 5 --retry-delay 3 -C - -o "$ZIP" "$URL2"
  fi
  got=$(sha_of "$ZIP")
  if [[ "$got" != "$EXPECT_SHA" ]]; then
    echo "[cosmocc] SHA256 mismatch: got $got want $EXPECT_SHA" >&2
    exit 1
  fi
}

ensure_zip
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/cosmocc-stage.XXXXXX")
BAK=$(mktemp -d "${TMPDIR:-/tmp}/cosmocc-bak.XXXXXX")
cleanup() { rm -rf "$STAGE" "$BAK"; }
trap cleanup EXIT

unzip -qo "$ZIP" -d "$STAGE"
test -x "$STAGE/bin/cosmocc"
got_bin=$(sha_of "$STAGE/bin/cosmocc")
if [[ "$got_bin" != "$EXPECT_BIN" ]]; then
  echo "[cosmocc] bin/cosmocc digest mismatch: got $got_bin want $EXPECT_BIN" >&2
  exit 1
fi
if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" && -f "$STAGE/bin/ape-m1.c" ]]; then
  cc -O2 -o "$STAGE/bin/ape" "$STAGE/bin/ape-m1.c"
  test -x "$STAGE/bin/ape"
fi
ver=$("$STAGE/bin/cosmocc" --version 2>/dev/null | head -1 || true)
if [[ -z "$ver" ]]; then
  echo "[cosmocc] empty --version from staged tree" >&2
  exit 1
fi

shopt -s nullglob
for p in "$DEST"/*; do
  base=$(basename "$p")
  [[ "$base" == "cosmocc-${VER}.zip" ]] && continue
  mv "$p" "$BAK/"
done
for p in "$STAGE"/*; do
  mv "$p" "$DEST/"
done
rmdir "$STAGE" 2>/dev/null || true
rm -rf "$BAK"
trap - EXIT

test -x "$DEST/bin/cosmocc"
test "$(sha_of "$DEST/bin/cosmocc")" = "$EXPECT_BIN"
echo "$DEST"
echo "[cosmocc] $ver"
