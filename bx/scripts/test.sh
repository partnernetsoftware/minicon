#!/bin/bash
# bx's own regression: Rust unit tests for the pure pieces, then the self-hosted
# suite (223 checks) through the freshly built binary.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release --offline
cargo test --release --offline

BIN="$ROOT/target/release/bx"
if [[ -f "$BIN.exe" ]]; then BIN="$BIN.exe"; fi
[[ -x "$BIN" || -f "$BIN" ]] || { echo "bx binary missing: $BIN" >&2; exit 2; }

BX_BIN="$BIN" "$BIN" run "$ROOT/tests/suite.bxs"

# Dynamic-value parity: generated runtime-parse programs must agree between the
# VM and the wasm backend (a fixed small run; the tool takes [cases] [seed]).
"$BIN" run "$ROOT/tools/wasm-diff-dyn.bxs" "$BIN" 12 20261118