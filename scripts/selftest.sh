#!/bin/bash
# Runs the repository's self-tests — the checks that guard the scripts rather
# than the product.
#
# They existed for months with nothing invoking them: not `build.sh`, not
# six-cell, not CI, not a line in AGENTS.md. A check nobody runs is not a gate,
# it is a file that looks like one, and the difference is invisible until
# something it was supposed to catch ships. All four offline ones passed on the
# day this entrypoint was written, so they were dormant rather than rotten.
#
#   scripts/selftest.sh            offline self-tests only
#   scripts/selftest.sh --court    also the ones that lease a VM
#
# Court self-tests are separate because leasing is slow and mutually exclusive
# with a release; they must never run by accident.
set -euo pipefail

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

WITH_COURT=false
case "${1:-}" in
  "") ;;
  --court) WITH_COURT=true ;;
  *)
    echo "usage: scripts/selftest.sh [--court]" >&2
    exit 2
    ;;
esac

OFFLINE=(
  scripts/publish-six-grid-runtime-selftest.sh
  scripts/six-grid-cloud-selftest.sh
  scripts/aggregate-six-grid-runtime-selftest.py
  scripts/cleanup-build-state-selftest.py
  scripts/product-source-hash-selftest.sh
)

COURT=(
  scripts/linux-utm-runner-selftest.sh
  scripts/utm-runner-registry-selftest.sh
  scripts/lib/utm-court-locator-selftest.sh
)

run_one() {
  local script="$1"
  printf '[selftest] %-52s ' "$script"
  local output
  case "$script" in
    *.py) output="$(python3 "$script" 2>&1)" || { printf 'FAIL\n%s\n' "$output" >&2; return 1; } ;;
    *) output="$(bash "$script" 2>&1)" || { printf 'FAIL\n%s\n' "$output" >&2; return 1; } ;;
  esac
  printf 'PASS\n'
}

failed=0
for script in "${OFFLINE[@]}"; do
  run_one "$script" || failed=1
done

if [ "$WITH_COURT" = true ]; then
  for script in "${COURT[@]}"; do
    run_one "$script" || failed=1
  done
else
  for script in "${COURT[@]}"; do
    printf '[selftest] %-52s SKIPPED (needs a court; pass --court)\n' "$script"
  done
fi

if [ "$failed" -ne 0 ]; then
  echo "[selftest] at least one self-test failed" >&2
  exit 1
fi
echo "[selftest] all requested self-tests passed"
