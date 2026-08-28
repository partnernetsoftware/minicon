#!/bin/bash
# Run already-linked MiniCon macOS artifacts inside a clean macOS guest.
# This target-side owner never invokes Cargo or reads a source checkout.

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: scripts/macos-runtime-qualify.sh TARGET_DIR status|test|throughput" >&2
  exit 2
fi

TARGET_DIR="$1"
MODE="$2"
if [ "$MODE" = throughput ]; then
  PROFILE="release-fast"
else
  PROFILE="debug"
fi
PROFILE_DIR="$TARGET_DIR/$PROFILE"
DEPS_DIR="$PROFILE_DIR/deps"
PRODUCT="$PROFILE_DIR/minicon"

[ -x "$PRODUCT" ] || {
  echo "missing MiniCon product executable: $PRODUCT" >&2
  exit 2
}

find_test_binary() {
  prefix="$1"
  found=""
  for candidate in "$DEPS_DIR"/"$prefix"-*; do
    [ -x "$candidate" ] || continue
    case "${candidate##*/}" in
      "$prefix"-[0-9a-f]*)
        list_output="$("$candidate" --list 2>/dev/null || true)"
        printf '%s\n' "$list_output" | tail -n 1 | grep -E '[0-9]+ tests?, [0-9]+ benchmarks?$' \
          >/dev/null 2>&1 || continue
        if [ -n "$found" ]; then
          echo "multiple Rust test harnesses for $prefix under $DEPS_DIR" >&2
          return 1
        fi
        found="$candidate"
        ;;
    esac
  done
  [ -n "$found" ] || {
    echo "missing Rust test harness for $prefix under $DEPS_DIR" >&2
    return 1
  }
  printf '%s\n' "$found"
}

run_test() {
  test_binary="$(find_test_binary "$1")"
  echo "[macos-runtime] RUN ${test_binary##*/}"
  "$test_binary" --test-threads=1 --nocapture
}

export AGENTERM_NO_ACTIVATE=1
export MINICON_TEST_BINARY="$PRODUCT"
if [ -d "$(dirname "$TARGET_DIR")/source" ]; then
  MINICON_REPO_ROOT="$(cd "$(dirname "$TARGET_DIR")/source" && pwd)"
  export MINICON_REPO_ROOT
fi

case "$MODE" in
  status)
    file "$PRODUCT"
    "$PRODUCT" --status
    ;;
  test)
    run_test minicon
    run_test minicon_core
    run_test minicon_alignment
    run_test minicon_load_portability
    run_test minicon_console_agent
    run_test minicon_control
    run_test minicon_blackbox
    ;;
  throughput)
    test_binary="$(find_test_binary minicon_throughput)"
    echo "[macos-runtime] RUN ${test_binary##*/} --ignored"
    "$test_binary" --ignored --test-threads=1 --nocapture
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    exit 2
    ;;
esac
