#!/bin/bash
# bx's own regression suite: build the host binary, then run the 51 checks on it.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release

BIN="$ROOT/target/release/bx"
if [[ -f "$BIN.exe" ]]; then BIN="$BIN.exe"; fi
[[ -x "$BIN" || -f "$BIN" ]] || { echo "bx binary missing: $BIN" >&2; exit 2; }

BX_BIN="$BIN" "$BIN" run "$ROOT/tests/suite.bxs"
