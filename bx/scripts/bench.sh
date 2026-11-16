#!/bin/bash
# Reproducible VM baseline: internal `now()` timing, so process start-up is not
# part of the number. Prints min/median per case; pass `--com <file.com>` to
# measure a packed artifact with the same scripts.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNS="${BX_BENCH_RUNS:-5}"
BIN="${BX_BENCH_BIN:-}"
if [[ "${1:-}" == "--com" ]]; then
    BIN="${2:?usage: bench.sh [--com <minicon-bx-*.com>]}"
    shift 2
fi
if [[ -z "$BIN" ]]; then
    cargo build --release --offline --manifest-path "$ROOT/Cargo.toml"
    BIN="$ROOT/target/release/bx"
    [[ -f "$BIN.exe" ]] && BIN="$BIN.exe"
fi
echo "bench: $BIN (runs=$RUNS)"
for case in fib loop calls; do
    times=()
    for ((i = 0; i < RUNS; i++)); do
        times+=("$("$BIN" run "$ROOT/tests/bench/$case.bxs")")
    done
    IFS=$'\n' sorted=($(printf '%s\n' "${times[@]}" | sort -n))
    count=${#sorted[@]}
    median=${sorted[$((count / 2))]}
    echo "$(printf '%-6s min=%6s ms  median=%6s ms' "$case" "${sorted[0]}" "$median")"
done