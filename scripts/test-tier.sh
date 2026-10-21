#!/bin/bash
# Tiered test entrypoint (owner directive 2026-10-16: suites must be layered
# by function so a small change does not run the whole pyramid every time;
# full runs belong around a release build).
#
# Usage: scripts/test-tier.sh <tier> [filter]
#        scripts/test-tier.sh --list|--plan|--profile NAME [--phase PHASE]
#   l0  lint            workspace/all-targets clippy -D warnings (no tests run)
#   l1  focused units   root/core/UI units + minicon_alignment + reuse consumer
#                       (not a claim of pure/no-process coverage)
#   l2  integration     minicon_control + minicon_mux + minicon_throughput +
#                       minicon_harness + minicon_console_agent +
#                       minicon_load_portability (+ linux variant on Linux) +
#                       policy suites (real MiniCon processes and GUI hosts)
#   l3  full local      fmt, workspace clippy, canonical build.sh test (workspace
#                       and vendored forks), reuse consumer and offline script
#                       self-tests. Native/VM/signed-artifact courts are separate.
#   all alias for l3.
#
# [filter] is an optional Rust test-name substring for l1/l2 only. A filtered
# run cannot be called full local qualification.
#
# Environment gates respected: CARGO_TARGET_DIR (use one outside the repo on
# developer hosts, e.g. D:/tmp/mc-target), MINICON_DISK_PRESSURE_GIB=0.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

if [[ "${1:-}" == --* ]]; then
  for python_bin in python3 python py; do
    if command -v "$python_bin" >/dev/null 2>&1; then
      exec "$python_bin" scripts/test-plan.py "$@"
    fi
  done
  echo "test-tier: Python interpreter not found" >&2
  exit 127
fi

tier=${1:-}
filter=${2:-}
if [[ $# -gt 2 ]]; then
  echo "test-tier: expected <tier> [filter]" >&2
  exit 2
fi

L1_SUITES=(minicon_alignment)
L2_SUITES=(
  minicon_control
  minicon_mux
  minicon_throughput
  minicon_harness
  minicon_console_agent
  minicon_load_portability
  authenticode_inspector_policy
  signing_workflow_policy
)
if [[ "$(uname -s)" == "Linux" ]]; then
  L2_SUITES+=(minicon_load_portability_linux)
fi

usage() {
  sed -n '2,20p' "$0" >&2
  exit 2
}

run_suite() {
  local suite=$1
  echo "==> ${tier}: ${suite} ${filter:+(filter: ${filter})}"
  if [[ -n "$filter" ]]; then
    cargo test --test "$suite" "$filter" -- --nocapture
  else
    cargo test --test "$suite"
  fi
}

run_lib_units() {
  echo "==> ${tier}: unit tests inside src/ and workspace crates ${filter:+(filter: ${filter})}"
  # The root package is bin-only; the workspace crates are libs. Three
  # invocations because cargo rejects a repeated --lib.
  cargo test --bin minicon ${filter:+"$filter"}
  cargo test -p minicon-core --lib ${filter:+"$filter"}
  cargo test -p minicon-ui-core --lib ${filter:+"$filter"}
}

case "$tier" in
  l0)
    cargo clippy --locked --workspace --all-targets -- -D warnings
    echo "l0 done: clippy clean"
    ;;
  l1)
    run_lib_units
    for suite in "${L1_SUITES[@]}"; do run_suite "$suite"; done
    bash ./scripts/check-reuse.sh
    echo "l1 done: unit + alignment contract"
    ;;
  l2)
    for suite in "${L2_SUITES[@]}"; do run_suite "$suite"; done
    echo "l2 done: process-level suites"
    ;;
  l3|all)
    if [[ -n "$filter" ]]; then
      echo "test-tier: l3/all requires an unfiltered run; use l1/l2 for focused feedback" >&2
      exit 2
    fi
    cargo fmt --all -- --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    bash ./scripts/build.sh test
    bash ./scripts/check-reuse.sh
    bash ./scripts/selftest.sh
    echo "l3 done: full local qualification (native and exact-artifact courts separate)"
    ;;
  *)
    usage
    ;;
esac
