#!/bin/bash
# Run already-linked MiniCon Linux test artifacts on a matching Linux host.
# This script intentionally does not invoke Cargo: the macOS owner cross-links
# the exact artifacts, while a Lima or remote runner supplies runtime evidence.

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: scripts/linux-runtime-qualify.sh TARGET_DIR status|logic|test|throughput" >&2
  exit 2
fi

TARGET_DIR="$1"
MODE="$2"
if [ "$MODE" = throughput ]; then
  PROFILE="release-fast"
else
  PROFILE="debug"
fi
DEPS_DIR="$TARGET_DIR/$PROFILE/deps"
PRODUCT="$TARGET_DIR/$PROFILE/minicon"

require_tool() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "required Linux runtime tool missing: $1" >&2
    exit 2
  }
}

require_tool file
require_tool timeout
[ -x "$PRODUCT" ] || {
  echo "missing MiniCon product executable: $PRODUCT" >&2
  exit 2
}
export MINICON_TEST_BINARY="$PRODUCT"

case "$MODE" in
  test|throughput)
    require_tool xvfb-run
    require_tool dbus-run-session
    ;;
esac

find_test_binary() {
  prefix="$1"
  newest=""
  for candidate in "$DEPS_DIR"/"$prefix"-*; do
    [ -x "$candidate" ] || continue
    if timeout 30s "$candidate" --list >/dev/null 2>&1; then
      if [ -z "$newest" ] || [ "$candidate" -nt "$newest" ]; then
        newest="$candidate"
      fi
    fi
  done
  if [ -n "$newest" ]; then
    printf '%s\n' "$newest"
    return 0
  fi
  echo "missing Rust test harness for $prefix under $DEPS_DIR" >&2
  return 1
}

run_test() {
  test_binary="$(find_test_binary "$1")"
  echo "[linux-runtime] RUN ${test_binary##*/}"
  "$test_binary" --test-threads=1 --nocapture
}

run_gui_test() {
  test_binary="$(find_test_binary "$1")"
  echo "[linux-runtime] RUN-X11 ${test_binary##*/}"
  timeout 600s xvfb-run -a -s "-screen 0 1280x900x24" \
    dbus-run-session -- "$test_binary" --test-threads=1 --nocapture
}

case "$MODE" in
  status)
    file "$PRODUCT"
    "$PRODUCT" --status
    ;;
  test)
    run_test minicon
    run_test minicon_core
    run_test minicon_alignment
    run_test minicon_console_agent
    run_test minicon_load_portability
    run_gui_test minicon_control
    run_gui_test minicon_blackbox
    run_gui_test minicon_accessibility_linux
    ;;
  logic)
    file "$PRODUCT"
    "$PRODUCT" --status
    run_test minicon
    run_test minicon_core
    run_test minicon_alignment
    ;;
  throughput)
    test_binary="$(find_test_binary minicon_throughput)"
    echo "[linux-runtime] RUN-X11 ${test_binary##*/} --ignored"
    timeout 600s xvfb-run -a -s "-screen 0 1280x900x24" \
      dbus-run-session -- "$test_binary" --ignored --test-threads=1 --nocapture
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    exit 2
    ;;
esac
