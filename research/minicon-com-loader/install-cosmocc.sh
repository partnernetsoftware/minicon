#!/bin/bash
# Always restore from the SHA-pinned 4.0.2 zip. Existing DEST/bin is not a pin;
# --version is GCC 14.1.0 and does not identify the Cosmopolitan release.
# Live tree is replaced by a same-parent rename swap of a complete NEXT tree.
# On any failure before commit, PREV (previous live) is restored intact.
set -euo pipefail
VER="${COSMOCC_VERSION:-4.0.2}"
DEST="${COSMOCC_DIR:-$HOME/cosmocc}"
ZIP_NAME="cosmocc-${VER}.zip"
EXPECT_SHA="${COSMOCC_SHA256:-85b8c37a406d862e656ad4ec14be9f6ce474c1b436b9615e91a55208aced3f44}"
EXPECT_BIN="${COSMOCC_BIN_SHA256:-eef9db8fabfc0c08f1930cbba87f60f69a1c49f28e4de006a1b0c6863e943e4b}"
URL1="https://github.com/jart/cosmopolitan/releases/download/${VER}/cosmocc-${VER}.zip"
URL2="https://cosmo.zip/pub/cosmocc/cosmocc-${VER}.zip"
mkdir -p "$DEST"
ZIP="$DEST/$ZIP_NAME"

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

committed=0
NEXT=""
PREV=""
cleanup() {
  if [[ "$committed" -eq 1 ]]; then
    if [[ -n "$PREV" && -d "$PREV" ]]; then rm -rf "$PREV"; fi
    return
  fi
  if [[ -n "$PREV" && -d "$PREV" ]]; then
    if [[ -e "$DEST" ]]; then rm -rf "$DEST"; fi
    mv "$PREV" "$DEST" || echo "[cosmocc] restore previous tree failed" >&2
  fi
  if [[ -n "$NEXT" && -d "$NEXT" ]]; then rm -rf "$NEXT"; fi
}
trap cleanup EXIT

ensure_zip
NEXT=$(mktemp -d "${DEST}.${VER}.next.XXXXXX")
if [[ -f "$ZIP" ]]; then
  ln "$ZIP" "$NEXT/$ZIP_NAME" 2>/dev/null || cp "$ZIP" "$NEXT/$ZIP_NAME"
fi
unzip -qo "$NEXT/$ZIP_NAME" -d "$NEXT"
test -x "$NEXT/bin/cosmocc"
got_bin=$(sha_of "$NEXT/bin/cosmocc")
if [[ "$got_bin" != "$EXPECT_BIN" ]]; then
  echo "[cosmocc] bin/cosmocc digest mismatch: got $got_bin want $EXPECT_BIN" >&2
  exit 1
fi
if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" && -f "$NEXT/bin/ape-m1.c" ]]; then
  cc -O2 -o "$NEXT/bin/ape" "$NEXT/bin/ape-m1.c"
  test -x "$NEXT/bin/ape"
fi
ver=$("$NEXT/bin/cosmocc" --version 2>/dev/null | head -1 || true)
if [[ -z "$ver" ]]; then
  echo "[cosmocc] empty --version from staged tree" >&2
  exit 1
fi

PREV="${DEST}.${VER}.prev.$$"
if [[ -e "$PREV" ]]; then
  echo "[cosmocc] leftover $PREV" >&2
  exit 1
fi
mv "$DEST" "$PREV"
if [[ "${COSMOCC_FAIL_SWAP:-}" == 1 ]]; then
  echo "[cosmocc] injected swap failure" >&2
  exit 2
fi
if ! mv "$NEXT" "$DEST"; then
  echo "[cosmocc] rename NEXT -> DEST failed" >&2
  exit 1
fi
committed=1
NEXT=""
rm -rf "$PREV"
PREV=""
trap - EXIT

test -x "$DEST/bin/cosmocc"
test "$(sha_of "$DEST/bin/cosmocc")" = "$EXPECT_BIN"
echo "$DEST"
echo "[cosmocc] $ver"
