#!/bin/bash
# Wasm-target soak / coverage report (PRD_02_43 G1).
#
#   bash bx/scripts/soak-wasm.sh [bx-binary]
#
# For every .bxs script in the local corpus it runs `bx check --target wasm`
# (compile-only) and, when the wasm target supports the script, compares its
# stdout with the VM's (`bx run` vs `bx run --target wasm`). Unsupported
# scripts are reported with the checker's reason and are not failures; a
# supported script whose outputs differ is a failure. Exit 0 when no supported
# script diverges, 1 otherwise.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/release/bx}"
if [[ -f "$BIN.exe" ]]; then BIN="$BIN.exe"; fi
[[ -x "$BIN" || -f "$BIN" ]] || { echo "bx binary missing: $BIN" >&2; exit 2; }

TMP="${TMPDIR:-${TEMP:-/tmp}}/bx-soak-wasm-$$"
mkdir -p "$TMP"
trap 'rm -rf "$TMP"' EXIT

ok=0
unsupported=0
diverged=0
failures=""
unsupported_list=""

corpus=()
while IFS= read -r file; do corpus+=("$file"); done < <(
  {
    find "$ROOT/examples" -maxdepth 1 -name '*.bxs' -type f 2>/dev/null || true
    find "$ROOT/tests/bench-wasm" -maxdepth 1 -name '*.bxs' -type f 2>/dev/null || true
  } | sort
)

if [[ ${#corpus[@]} -eq 0 ]]; then
  echo "soak-wasm: no corpus scripts found under bx/examples or bx/tests/bench-wasm" >&2
  exit 2
fi

index=0
for script in "${corpus[@]}"; do
  index=$((index + 1))
  name="${script#"$ROOT"/}"
  if grep -qE '\.ms|now\(\)|performance_now|elapsed' "$script"; then
    unsupported=$((unsupported + 1))
    unsupported_list+="  - $name: timing/environment output (skipped)"$'\n'
    continue
  fi
  check_output="$("$BIN" check --target wasm --json "$script" 2>&1 || true)"
  if [[ "$check_output" != *'"ok":true'* ]]; then
    unsupported=$((unsupported + 1))
    reason="${check_output#*\"error\":\"}"
    reason="${reason%%\"*}"
    unsupported_list+="  - $name: $reason"$'\n'
    continue
  fi
  module="$TMP/case-$index.wasm"
  if ! "$BIN" build --target wasm "$script" -o "$module" >/dev/null 2>"$TMP/build.err"; then
    diverged=$((diverged + 1))
    failures+="  - $name: build failed: $(head -n 1 "$TMP/build.err")"$'\n'
    continue
  fi
  vm_out="$("$BIN" run "$script" 2>"$TMP/vm.err" || true)"
  vm_out2="$("$BIN" run "$script" 2>"$TMP/vm2.err" || true)"
  if [[ "$vm_out" != "$vm_out2" ]]; then
    # Scripts that embed timing or environment state are not byte-stable.
    unsupported=$((unsupported + 1))
    unsupported_list+="  - $name: nondeterministic VM output (skipped)"$'\n'
    continue
  fi
  wasm_out="$("$BIN" run --target wasm "$script" 2>"$TMP/wasm.err" || true)"
  if [[ "$vm_out" == "$wasm_out" ]]; then
    ok=$((ok + 1))
  else
    diverged=$((diverged + 1))
    failures+="  - $name: outputs differ"$'\n'
  fi
done

echo "soak-wasm: ${#corpus[@]} scripts, $ok supported+matched, $unsupported unsupported, $diverged diverged"
if [[ -n "$unsupported_list" ]]; then
  echo "unsupported (informational):"
  printf '%s' "$unsupported_list"
fi
if [[ $diverged -ne 0 ]]; then
  echo "diverged:"
  printf '%s' "$failures"
  exit 1
fi
exit 0
