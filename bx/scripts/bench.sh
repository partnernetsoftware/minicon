#!/bin/bash
# Reproducible VM baseline: internal `now()` timing, so process start-up is not
# part of the number. Prints min/median per case; pass `--com <file.com>` to
# measure a packed artifact with the same scripts.
#
# `--wasm` compares the VM against the wasm backend on the shared numeric
# subset (tests/bench-wasm), wall clock including process start so both sides
# pay the same launch cost.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNS="${BX_BENCH_RUNS:-5}"
BIN="${BX_BENCH_BIN:-}"
MODE="${1:-}"
if [[ "$MODE" == "--com" ]]; then
    BIN="${2:?usage: bench.sh [--com <minicon-bx-*.com>] | --wasm [--com <file>]}"
    shift 2
fi
if [[ -z "$BIN" ]]; then
    cargo build --release --offline --manifest-path "$ROOT/Cargo.toml"
    BIN="$ROOT/target/release/bx"
    [[ -f "$BIN.exe" ]] && BIN="$BIN.exe"
fi
echo "bench: $BIN (runs=$RUNS)"

in_ms() {
    # seconds with nanoseconds -> integer milliseconds
    local start=$1 end=$2
    python - "$start" "$end" <<'PY'
import sys
start, end = float(sys.argv[1]), float(sys.argv[2])
print(int((end - start) * 1000))
PY
}
now_s() {
    date +%s.%N
}

if [[ "$MODE" == "--wasm" ]]; then
    TMP="$ROOT/target/bench-wasm"
    mkdir -p "$TMP"
    echo "wasm backend vs VM (wall clock, process start included)"
    for case in fib loop calls coll recs boolarr; do
        src="$ROOT/tests/bench-wasm/$case.bxs"
        mod="$TMP/$case.wasm"
        "$BIN" build --wasm "$src" -o "$mod" >/dev/null 2>&1
        vm_times=()
        wasm_times=()
        for ((i = 0; i < RUNS; i++)); do
            start=$(now_s); "$BIN" run "$src" >/dev/null; end=$(now_s)
            vm_times+=("$(in_ms "$start" "$end")")
            start=$(now_s); "$BIN" run "$mod" >/dev/null; end=$(now_s)
            wasm_times+=("$(in_ms "$start" "$end")")
        done
        min_of() { printf '%s\n' "$@" | sort -n | head -1; }
        echo "$(printf '%-6s vm=%5s ms  wasm=%5s ms' "$case" "$(min_of "${vm_times[@]}")" "$(min_of "${wasm_times[@]}")")"
    done
    exit 0
fi

for case in fib loop calls map; do
    times=()
    for ((i = 0; i < RUNS; i++)); do
        times+=("$("$BIN" run "$ROOT/tests/bench/$case.bxs")")
    done
    IFS=$'\n' sorted=($(printf '%s\n' "${times[@]}" | sort -n))
    count=${#sorted[@]}
    median=${sorted[$((count / 2))]}
    echo "$(printf '%-6s min=%6s ms  median=%6s ms' "$case" "${sorted[0]}" "$median")"
done