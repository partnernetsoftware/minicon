#!/usr/bin/env bash
# Reproduce the absorption evidence bundle for minicon-bx.com.
#
#   bash bx/scripts/evidence.sh [path/to/bx]
#
# Runs the bx suite, the verifier, the five differential tools and the wasm
# soak, and prints one PASS/FAIL line per stage. Exit status is non-zero when any
# stage fails. The default binary is the release build under bx/target.
set -uo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
bx="${1:-$here/target/release/bx.exe}"
if [ ! -x "$bx" ]; then
  echo "evidence: cannot execute '$bx' (pass the bx binary as the first argument)" >&2
  exit 2
fi

failures=0
stage() {
  local name="$1"
  shift
  local out
  if out="$("$@" 2>&1)"; then
    local tail
    tail="$(printf '%s\n' "$out" | grep -E 'PASS|OK|matched' | tail -n 1)"
    printf 'PASS  %-24s %s\n' "$name" "${tail:-ok}"
  else
    failures=$((failures + 1))
    printf 'FAIL  %-24s\n' "$name"
    printf '%s\n' "$out" | tail -n 15 | sed 's/^/      /'
  fi
}

stage "suite" "$bx" run "$here/tests/suite.bxs"
stage "verifier" "$bx" test "$here/tools/verify.bxs" "$bx"
stage "diff" "$bx" run "$here/tools/wasm-diff.bxs" "$bx" 60 21
stage "diff-dyn" "$bx" run "$here/tools/wasm-diff-dyn.bxs" "$bx" 80 22
stage "diff-methods" "$bx" run "$here/tools/wasm-diff-methods.bxs" "$bx" 60 23
stage "diff-coll" "$bx" run "$here/tools/wasm-diff-coll.bxs" "$bx" 60 24
stage "diff-attempt" "$bx" run "$here/tools/wasm-diff-attempt.bxs" "$bx" 24 25
stage "soak-wasm" bash "$here/scripts/soak-wasm.sh" "$bx"

if [ "$failures" -ne 0 ]; then
  echo "evidence: $failures stage(s) failed" >&2
  exit 1
fi
echo "evidence: all stages passed"