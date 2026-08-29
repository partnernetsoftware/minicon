#!/bin/bash
# Stage six-cell payloads + compile trampoline.
# cosmocc → dist/minicon.com ; else host cc → dist/minicon-loader (needs MINICON_COM_CELLS).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BUILD="${MINICON_SIX_CELL_BUILD:-$ROOT/target-six/builds/current}"
FRESH="$HERE/payload-build"
COSMO="${COSMOCC_DIR:-$HOME/cosmocc}"
DIST="$HERE/dist"
CELLS="$DIST/cells"

payload() {
  cell="$1"
  rel="$2"
  if [[ -f "$FRESH/$rel" ]]; then
    echo "$FRESH/$rel"
  else
    echo "$BUILD/$rel"
  fi
}

stage() {
  cell="$1"
  src="$2"
  leaf="$3"
  mkdir -p "$CELLS/$cell"
  if [[ ! -f "$src" ]]; then
    echo "missing $src" >&2
    exit 1
  fi
  cp "$src" "$CELLS/$cell/$leaf"
}

rm -rf "$DIST"
mkdir -p "$CELLS"

stage osx-aarch64 "$(payload osx-aarch64 osx-aarch64/aarch64-apple-darwin/release-fast/minicon)" minicon
stage osx-x86_64 "$(payload osx-x86_64 osx-x86_64/x86_64-apple-darwin/release-fast/minicon)" minicon
stage lnx-aarch64 "$(payload lnx-aarch64 lnx-aarch64/aarch64-unknown-linux-gnu/release-fast/minicon)" minicon
stage lnx-x86_64 "$(payload lnx-x86_64 lnx-x86_64/x86_64-unknown-linux-gnu/release-fast/minicon)" minicon
stage win-aarch64 "$(payload win-aarch64 win-aarch64/aarch64-pc-windows-msvc/release-fast/minicon.exe)" minicon.exe
stage win-x86_64 "$(payload win-x86_64 win-x86_64/x86_64-pc-windows-msvc/release-fast/minicon.exe)" minicon.exe

if [[ -x "$COSMO/bin/cosmocc" ]]; then
  echo "[pack] cosmocc → dist/minicon.com"
  "$COSMO/bin/cosmocc" -Os -static -o "$DIST/minicon.com" "$HERE/loader.c"
  (
    cd "$DIST"
    zip -q -r minicon.com cells
    zip -A minicon.com
  )
  echo "[pack] zip overlay cells/ + zipalign"
else
  echo "[pack] no cosmocc; host-cc dispatcher"
  cc -O2 -o "$DIST/minicon-loader" "$HERE/loader.c"
fi

echo "[pack] staged:"
find "$CELLS" -type f -exec ls -lh {} \;
echo "[pack] OK $DIST"
