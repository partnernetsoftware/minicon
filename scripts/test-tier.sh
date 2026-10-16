#!/bin/bash
# Tiered test entrypoint (owner directive 2026-10-16: suites must be layered
# by function so a small change does not run the whole pyramid every time;
# full runs belong around a release build).
#
# Usage: scripts/test-tier.sh <tier> [filter]
#   l0  lint            clippy --all-targets -D warnings (no tests run)
#   l1  unit+contract   unit tests in src/ + minicon_alignment (pure, fast,
#                       no GUI, no spawned processes)
#   l2  process-level   minicon_control + minicon_mux + minicon_throughput +
#                       minicon_harness + minicon_console_agent +
#                       minicon_load_portability (+ linux variant on Linux) +
#                       policy suites (spawns real minicon processes, no GUI
#                       window)
#   l3  full            everything: l0+l1+l2 plus minicon_blackbox (the only
#                       suite that opens a real GUI window via
#                       gui_test_guard). This is the release-gate tier.
#   all alias for l3.
#
# [filter] is an optional Rust test-name substring passed to the suites that
# run in tiers l1/l2/l3.
#
# Environment gates respected: CARGO_TARGET_DIR (use one outside the repo on
# developer hosts, e.g. D:/tmp/mc-target), MINICON_DISK_PRESSURE_GIB=0.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

tier=${1:-}
filter=${2:-}

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
    cargo clippy --all-targets -- -D warnings
    echo "l0 done: clippy clean"
    ;;
  l1)
    run_lib_units
    for suite in "${L1_SUITES[@]}"; do run_suite "$suite"; done
    echo "l1 done: unit + alignment contract"
    ;;
  l2)
    for suite in "${L2_SUITES[@]}"; do run_suite "$suite"; done
    echo "l2 done: process-level suites"
    ;;
  l3|all)
    cargo clippy --all-targets -- -D warnings
    run_lib_units
    for suite in "${L1_SUITES[@]}" "${L2_SUITES[@]}"; do run_suite "$suite"; done
    run_suite minicon_blackbox
    echo "l3 done: full release-gate pyramid"
    ;;
  *)
    usage
    ;;
esac
