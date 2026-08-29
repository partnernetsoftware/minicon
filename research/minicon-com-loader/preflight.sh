#!/bin/bash
# Checks cosmocc + six-cell payloads. No guess, no install.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD="${MINICON_SIX_CELL_BUILD:-$ROOT/target-six/builds/current}"
COSMO="${COSMOCC_DIR:-$HOME/cosmocc}"
fail=0

need() {
  rel="$1"
  p="$BUILD/$rel"
  if [[ -f "$p" ]]; then
    sz=$(wc -c <"$p" | tr -d ' ')
    printf 'OK   %10s  %s\n' "$sz" "$rel"
  else
    printf 'MISS              %s\n' "$rel"
    fail=1
  fi
}

echo "repo=$ROOT"
echo "build=$BUILD"
if [[ -x "$COSMO/bin/cosmocc" ]]; then
  echo "cosmocc=$COSMO/bin/cosmocc"
else
  echo "cosmocc=MISSING ($COSMO/bin/cosmocc) — pack.sh will host-cc only"
fi

echo "-- release-fast payloads --"
need osx-aarch64/aarch64-apple-darwin/release-fast/minicon
need osx-x86_64/x86_64-apple-darwin/release-fast/minicon
need lnx-aarch64/aarch64-unknown-linux-gnu/release/minicon
need lnx-x86_64/x86_64-unknown-linux-gnu/release/minicon
need win-aarch64/aarch64-pc-windows-msvc/release-fast/minicon.exe
need win-x86_64/x86_64-pc-windows-msvc/release-fast/minicon.exe

exit "$fail"
