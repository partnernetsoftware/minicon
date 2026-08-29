#!/bin/bash
# Build and test MiniCon's six target cells from one Apple Silicon macOS host.
# Cross cells always link every Cargo target. Runtime stages run only when this
# machine has a compatible required runner; absence is BLOCKED. Lima is an
# explicit optional accelerator and records NOT_REQUESTED when disabled.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LIMA_COURT="${MINICON_LIMA_COURT_CLI:-$SCRIPT_DIR/lima-court.sh}"
LIMA_ACCELERATOR="${MINICON_ENABLE_LIMA_ACCELERATOR:-0}"
OUT_DIR="${MINICON_SIX_CELL_OUT:-$REPO_ROOT/target-six}"
RESULTS="$OUT_DIR/results.tsv"
RECEIPT="$OUT_DIR/receipt.json"
mkdir -p "$OUT_DIR/logs"
: >"$RESULTS"

cd "$REPO_ROOT" || exit 2
export AGENTERM_NO_ACTIVATE=1

# Reclaim only expired, unreferenced snapshots. The cleaner protects the
# current symlink, receipt-owned root, newest snapshots and active markers.
python3 scripts/cleanup-build-state.py --apply --scope six-cell

record() {
  results_file="${RESULT_SINK:-$RESULTS}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" >>"$results_file"
}

run_stage() {
  cell="$1"
  stage="$2"
  shift 2
  log="$OUT_DIR/logs/${cell}-${stage}.log"
  started="$(date +%s)"
  printf '[six-cell] %-14s %-18s START\n' "$cell" "$stage"
  if "$@" >"$log" 2>&1; then
    status="PASS"
    rc=0
  else
    rc=$?
    status="FAIL"
  fi
  elapsed="$(( $(date +%s) - started ))"
  printf '[six-cell] %-14s %-18s %s (%ss)\n' "$cell" "$stage" "$status" "$elapsed"
  record "$cell" "$stage" "$status" "$elapsed" "target-six/logs/${cell}-${stage}.log" "exit=$rc"
  return 0
}

blocked() {
  cell="$1"
  stage="$2"
  reason="$3"
  printf '[six-cell] %-14s %-18s BLOCKED (%s)\n' "$cell" "$stage" "$reason"
  record "$cell" "$stage" "BLOCKED" "0" "" "$reason"
}

not_requested() {
  cell="$1"
  stage="$2"
  reason="$3"
  printf '[six-cell] %-14s %-18s NOT_REQUESTED (%s)\n' "$cell" "$stage" "$reason"
  record "$cell" "$stage" "NOT_REQUESTED" "0" "" "$reason"
}

lima_running() {
  instance="$1"
  command -v limactl >/dev/null 2>&1 &&
    limactl list --json "$instance" 2>/dev/null |
      python3 -c 'import json,sys; value=json.load(sys.stdin); raise SystemExit(0 if value.get("status") == "Running" else 1)' \
        >/dev/null 2>&1
}

lima_stop_if_running() {
  instance="$1"
  if lima_running "$instance"; then
    limactl stop "$instance" >/dev/null 2>&1 || true
  fi
}

lima_start_if_needed() {
  instance="$1"
  lima_running "$instance" && return 0
  command -v limactl >/dev/null 2>&1 || return 1
  limactl start "$instance" >/dev/null 2>&1 || return 1
  for _ in $(seq 1 120); do
    lima_running "$instance" && return 0
    sleep 1
  done
  return 1
}

run_lima_linux_stage() {
  cell="$1"
  stage="$2"
  instance="$3"
  target_dir="$4"
  mode="$5"
  case "$cell:$instance" in
    lnx-x86_64:"$LNX_X86_64_KERNEL_LIMA") court=lnx-x86_64-kernel ;;
    lnx-aarch64:*) court=lnx-aarch64-fast ;;
    *) court=lnx-x86_64-rosetta ;;
  esac
  run_stage "$cell" "$stage" "$LIMA_COURT" exec "$court" -- \
    bash -lc "cd '$REPO_ROOT' && scripts/linux-runtime-qualify.sh '$target_dir' '$mode'"
}

run_windows_runner_stage() {
  cell="$1"
  stage="$2"
  runner="$3"
  target_dir="$4"
  mode="$5"
  run_stage "$cell" "$stage" "$runner" "$cell" "$target_dir" "$mode"
}

run_macos_runner_stage() {
  cell="$1"
  stage="$2"
  runner="$3"
  target_dir="$4"
  mode="$5"
  run_stage "$cell" "$stage" "$runner" osx-aarch64 "$target_dir" "$mode"
}

run_linux_desktop_runner_stage() {
  cell="$1"
  stage="$2"
  runner="$3"
  target_dir="$4"
  mode="$5"
  run_stage "$cell" "$stage" "$runner" "$cell" "$target_dir" "$mode"
}

stop_linux_desktop_runner() {
  cell="$1"; runner="$2"
  [ -n "$runner" ] && [ -x "$runner" ] || return 0
  "$runner" "$cell" . stop
}

stop_macos_runner() {
  if [ -n "${MINICON_MACOS_AARCH64_RUNNER:-}" ] &&
     [ -x "$MINICON_MACOS_AARCH64_RUNNER" ]; then
    "$MINICON_MACOS_AARCH64_RUNNER" osx-aarch64 . stop
  fi
}

stop_windows_runners() {
  rc=0
  if [ -n "${MINICON_WIN_X86_64_RUNNER:-}" ] && [ -x "$MINICON_WIN_X86_64_RUNNER" ]; then
    "$MINICON_WIN_X86_64_RUNNER" win-x86_64 . stop || rc=1
  fi
  if [ -n "${MINICON_WIN_AARCH64_RUNNER:-}" ] && [ -x "$MINICON_WIN_AARCH64_RUNNER" ]; then
    "$MINICON_WIN_AARCH64_RUNNER" win-aarch64 . stop || rc=1
  fi
  return "$rc"
}

inspect_artifact() {
  cell="$1"
  path="$2"
  expected="$3"
  log="$OUT_DIR/logs/${cell}-artifact.log"
  if [ ! -f "$path" ]; then
    printf 'missing artifact: %s\n' "$path" >"$log"
    status="FAIL"
  else
    file "$path" >"$log" 2>&1
    if grep -F "$expected" "$log" >/dev/null 2>&1; then
      status="PASS"
    else
      status="FAIL"
    fi
  fi
  printf '[six-cell] %-14s %-18s %s\n' "$cell" "artifact" "$status"
  record "$cell" "artifact" "$status" "0" "target-six/logs/${cell}-artifact.log" "$expected"
}

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'required tool missing: %s\n' "$1" >&2
    exit 2
  fi
}

require_tool cargo
require_tool cargo-xwin
require_tool cargo-zigbuild
require_tool python3

case "$LIMA_ACCELERATOR" in
  0|1) ;;
  *) printf 'MINICON_ENABLE_LIMA_ACCELERATOR must be 0 or 1\n' >&2; exit 2 ;;
esac

BUILD_JOBS="${MINICON_BUILD_JOBS:-5}"
CARGO_JOBS_PER_CELL="${MINICON_CARGO_JOBS_PER_CELL:-2}"
case "$BUILD_JOBS:$CARGO_JOBS_PER_CELL" in
  *[!0-9:]*|0:*|*:0) printf 'build concurrency must be positive integers\n' >&2; exit 2 ;;
esac
export CARGO_BUILD_JOBS="$CARGO_JOBS_PER_CELL"

SOURCE_STATE_START="$(python3 scripts/source-fingerprint.py)"
BUILD_DIR="${MINICON_SIX_CELL_BUILD_DIR:-$OUT_DIR/builds/current}"
BUILD_REL="${BUILD_DIR#"$REPO_ROOT"/}"
mkdir -p "$BUILD_DIR"
BUILD_ACTIVE_MARKER="$BUILD_DIR/.minicon-build-active"
printf '%s\n' "$$" >"$BUILD_ACTIVE_MARKER"
cleanup_qualification() {
  rm -f "$BUILD_ACTIVE_MARKER"
  if [ "$LIMA_ACCELERATOR" = 1 ]; then
    # Reap resolves the actual active court before stopping it. Releasing all
    # aliases in a fixed order could finalize one court while stopping a
    # different instance because two logical courts share the ARM64 guest.
    "$LIMA_COURT" reap >/dev/null 2>&1 || true
    lima_stop_if_running "$LNX_X86_64_LIMA"
    lima_stop_if_running "$LNX_X86_64_KERNEL_LIMA"
    lima_stop_if_running "$LNX_AARCH64_LIMA"
  fi
}
trap cleanup_qualification EXIT
trap 'cleanup_qualification; exit 130' HUP INT TERM

LNX_X86_64_LIMA="${MINICON_LNX_X86_64_LIMA:-minicon-lnx-aarch64}"
LNX_X86_64_KERNEL_LIMA="${MINICON_LNX_X86_64_KERNEL_LIMA:-minicon-lnx-x86_64}"
LNX_AARCH64_LIMA="${MINICON_LNX_AARCH64_LIMA:-minicon-lnx-aarch64}"

# Runtime guests are mutually scheduled test targets. Keep both Linux guests
# down while macOS builds and Windows UTM courts own the host's CPU and memory.
if [ "$LIMA_ACCELERATOR" = 1 ]; then
  lima_stop_if_running "$LNX_X86_64_LIMA"
  lima_stop_if_running "$LNX_X86_64_KERNEL_LIMA"
  lima_stop_if_running "$LNX_AARCH64_LIMA"
fi

run_stage common fmt cargo fmt --all -- --check

build_osx_aarch64() {
  run_stage osx-aarch64 clippy env CARGO_TARGET_DIR="$BUILD_DIR/osx-aarch64" \
    cargo clippy --locked --workspace --all-targets --target aarch64-apple-darwin -- -D warnings
  run_stage osx-aarch64 test env CARGO_TARGET_DIR="$BUILD_DIR/osx-aarch64" \
    cargo test --locked --workspace --all-targets --target aarch64-apple-darwin
  run_stage osx-aarch64 throughput env CARGO_TARGET_DIR="$BUILD_DIR/osx-aarch64" \
    cargo test --locked --profile release-fast --target aarch64-apple-darwin \
      --test minicon_throughput -- --ignored --nocapture
  inspect_artifact osx-aarch64 "$BUILD_DIR/osx-aarch64/aarch64-apple-darwin/debug/minicon" "Mach-O 64-bit executable arm64"
}

build_osx_x86_64() {
  run_stage osx-x86_64 clippy env CARGO_TARGET_DIR="$BUILD_DIR/osx-x86_64" \
    cargo clippy --locked --workspace --all-targets --target x86_64-apple-darwin -- -D warnings
  run_stage osx-x86_64 test-link env CARGO_TARGET_DIR="$BUILD_DIR/osx-x86_64" \
    cargo test --locked --workspace --all-targets --target x86_64-apple-darwin --no-run
  inspect_artifact osx-x86_64 "$BUILD_DIR/osx-x86_64/x86_64-apple-darwin/debug/minicon" "Mach-O 64-bit executable x86_64"
  if arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
    run_stage osx-x86_64 rosetta-proof bash -c \
      '[ "$(arch -x86_64 /usr/bin/uname -m)" = x86_64 ] && [ "$(arch -x86_64 /usr/sbin/sysctl -n sysctl.proc_translated)" = 1 ]'
    run_stage osx-x86_64 test env CARGO_TARGET_DIR="$BUILD_DIR/osx-x86_64" \
      cargo test --locked --workspace --all-targets --target x86_64-apple-darwin
    run_stage osx-x86_64 throughput env CARGO_TARGET_DIR="$BUILD_DIR/osx-x86_64" \
      cargo test --locked --profile release-fast --target x86_64-apple-darwin \
        --test minicon_throughput -- --ignored --nocapture
  else
    blocked osx-x86_64 test "Rosetta 2 is not installed"
    blocked osx-x86_64 throughput "Rosetta 2 is not installed"
  fi
}

build_win_x86_64() {
  run_stage win-x86_64 all-target-link env CARGO_TARGET_DIR="$BUILD_DIR/win-x86_64" \
    cargo xwin build --locked --workspace --all-targets --target x86_64-pc-windows-msvc
  run_stage win-x86_64 throughput-link env CARGO_TARGET_DIR="$BUILD_DIR/win-x86_64" \
    cargo xwin build --locked --profile release-fast --workspace --all-targets \
      --target x86_64-pc-windows-msvc
  inspect_artifact win-x86_64 "$BUILD_DIR/win-x86_64/x86_64-pc-windows-msvc/debug/minicon.exe" "GUI) x86-64"
}

build_win_aarch64() {
  run_stage win-aarch64 all-target-link env CARGO_TARGET_DIR="$BUILD_DIR/win-aarch64" \
    cargo xwin build --locked --workspace --all-targets --target aarch64-pc-windows-msvc
  run_stage win-aarch64 throughput-link env CARGO_TARGET_DIR="$BUILD_DIR/win-aarch64" \
    cargo xwin build --locked --profile release-fast --workspace --all-targets \
      --target aarch64-pc-windows-msvc
  inspect_artifact win-aarch64 "$BUILD_DIR/win-aarch64/aarch64-pc-windows-msvc/debug/minicon.exe" "GUI) Aarch64"
}

build_lnx_x86_64() {
  run_stage lnx-x86_64 all-target-link env CARGO_TARGET_DIR="$BUILD_DIR/lnx-x86_64" \
    cargo zigbuild --locked --workspace --all-targets --target x86_64-unknown-linux-gnu
  run_stage lnx-x86_64 throughput-link env CARGO_TARGET_DIR="$BUILD_DIR/lnx-x86_64" \
    cargo zigbuild --locked --profile release-fast --workspace --all-targets \
      --target x86_64-unknown-linux-gnu
  inspect_artifact lnx-x86_64 "$BUILD_DIR/lnx-x86_64/x86_64-unknown-linux-gnu/debug/minicon" "ELF 64-bit LSB pie executable, x86-64"
}

build_lnx_aarch64() {
  run_stage lnx-aarch64 all-target-link env CARGO_TARGET_DIR="$BUILD_DIR/lnx-aarch64" \
    cargo zigbuild --locked --workspace --all-targets --target aarch64-unknown-linux-gnu
  run_stage lnx-aarch64 throughput-link env CARGO_TARGET_DIR="$BUILD_DIR/lnx-aarch64" \
    cargo zigbuild --locked --profile release-fast --workspace --all-targets \
      --target aarch64-unknown-linux-gnu
  inspect_artifact lnx-aarch64 "$BUILD_DIR/lnx-aarch64/aarch64-unknown-linux-gnu/debug/minicon" "ELF 64-bit LSB pie executable, ARM aarch64"
}

build_windows() {
  build_win_x86_64
  build_win_aarch64
}

# cargo-xwin owns a shared host cache and races while installing its clang-cl
# shim when two targets start together on a fresh machine. Keep the two Windows
# cells in one worker; the other four target directories are independent.
build_groups=(osx-aarch64 osx-x86_64 windows lnx-x86_64 lnx-aarch64)
build_functions=(build_osx_aarch64 build_osx_x86_64 build_windows build_lnx_x86_64 build_lnx_aarch64)
result_dir="$OUT_DIR/results.d"
mkdir -p "$result_dir"
pids=()
wait_build_batch() {
  for pid in "${pids[@]}"; do wait "$pid"; done
  pids=()
}
build_started="$(date +%s)"
printf '[six-cell] build fan-out: groups=%s cargo-jobs-per-group=%s\n' "$BUILD_JOBS" "$CARGO_JOBS_PER_CELL"
for index in 0 1 2 3 4; do
  cell="${build_groups[$index]}"
  function_name="${build_functions[$index]}"
  (RESULT_SINK="$result_dir/$cell.tsv"; : >"$RESULT_SINK"; "$function_name") &
  pids+=("$!")
  if [ "${#pids[@]}" -ge "$BUILD_JOBS" ]; then wait_build_batch; fi
done
[ "${#pids[@]}" -eq 0 ] || wait_build_batch
for cell in "${build_groups[@]}"; do cat "$result_dir/$cell.tsv" >>"$RESULTS"; done
build_elapsed="$(( $(date +%s) - build_started ))"
record common build-fanout PASS "$build_elapsed" "" \
  "groups=5,max-parallel=$BUILD_JOBS,cargo-jobs-per-group=$CARGO_JOBS_PER_CELL"

# The clean macOS guest is a release/permission court for the ARM64 artifact,
# not a seventh architecture cell. It consumes the exact host-linked bytes and
# returns to an idle state before Rosetta or another runtime guest is scheduled.
if [ -n "${MINICON_MACOS_AARCH64_RUNNER:-}" ] &&
   [ -x "$MINICON_MACOS_AARCH64_RUNNER" ]; then
  run_macos_runner_stage osx-aarch64 clean-runtime-status \
    "$MINICON_MACOS_AARCH64_RUNNER" \
    "$BUILD_REL/osx-aarch64/aarch64-apple-darwin" status
  run_macos_runner_stage osx-aarch64 clean-test \
    "$MINICON_MACOS_AARCH64_RUNNER" \
    "$BUILD_REL/osx-aarch64/aarch64-apple-darwin" test
  run_macos_runner_stage osx-aarch64 clean-throughput \
    "$MINICON_MACOS_AARCH64_RUNNER" \
    "$BUILD_REL/osx-aarch64/aarch64-apple-darwin" throughput
  run_stage common macos-clean-idle stop_macos_runner
else
  blocked osx-aarch64 clean-runtime-status \
    "MINICON_MACOS_AARCH64_RUNNER is not configured"
  blocked osx-aarch64 clean-test \
    "MINICON_MACOS_AARCH64_RUNNER is not configured"
  blocked osx-aarch64 clean-throughput \
    "MINICON_MACOS_AARCH64_RUNNER is not configured"
fi

if [ -n "${MINICON_WIN_X86_64_RUNNER:-}" ] && [ -x "$MINICON_WIN_X86_64_RUNNER" ]; then
  run_windows_runner_stage win-x86_64 runtime-status "$MINICON_WIN_X86_64_RUNNER" \
    "$BUILD_REL/win-x86_64/x86_64-pc-windows-msvc" status
  run_windows_runner_stage win-x86_64 test "$MINICON_WIN_X86_64_RUNNER" \
    "$BUILD_REL/win-x86_64/x86_64-pc-windows-msvc" test
  run_windows_runner_stage win-x86_64 throughput "$MINICON_WIN_X86_64_RUNNER" \
    "$BUILD_REL/win-x86_64/x86_64-pc-windows-msvc" throughput
else
  blocked win-x86_64 runtime-status "MINICON_WIN_X86_64_RUNNER is not configured"
  blocked win-x86_64 test "MINICON_WIN_X86_64_RUNNER is not configured"
  blocked win-x86_64 throughput "MINICON_WIN_X86_64_RUNNER is not configured"
fi

run_stage common windows-cell-boundary stop_windows_runners

if [ -n "${MINICON_WIN_AARCH64_RUNNER:-}" ] && [ -x "$MINICON_WIN_AARCH64_RUNNER" ]; then
  run_windows_runner_stage win-aarch64 runtime-status "$MINICON_WIN_AARCH64_RUNNER" \
    "$BUILD_REL/win-aarch64/aarch64-pc-windows-msvc" status
  run_windows_runner_stage win-aarch64 test "$MINICON_WIN_AARCH64_RUNNER" \
    "$BUILD_REL/win-aarch64/aarch64-pc-windows-msvc" test
  run_windows_runner_stage win-aarch64 throughput "$MINICON_WIN_AARCH64_RUNNER" \
    "$BUILD_REL/win-aarch64/aarch64-pc-windows-msvc" throughput
else
  blocked win-aarch64 runtime-status "MINICON_WIN_AARCH64_RUNNER is not configured"
  blocked win-aarch64 test "MINICON_WIN_AARCH64_RUNNER is not configured"
  blocked win-aarch64 throughput "MINICON_WIN_AARCH64_RUNNER is not configured"
fi

run_stage common windows-idle stop_windows_runners

if [ "$LIMA_ACCELERATOR" = 1 ]; then
  "$LIMA_COURT" lease lnx-x86_64-rosetta >/dev/null 2>&1 || true
  if lima_running "$LNX_X86_64_LIMA"; then
    run_lima_linux_stage lnx-x86_64 runtime-status "$LNX_X86_64_LIMA" \
      "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" status
    run_lima_linux_stage lnx-x86_64 test "$LNX_X86_64_LIMA" \
      "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" test
    run_lima_linux_stage lnx-x86_64 throughput "$LNX_X86_64_LIMA" \
      "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" throughput
  else
    blocked lnx-x86_64 runtime-status "Lima instance $LNX_X86_64_LIMA is not running"
    blocked lnx-x86_64 test "Lima instance $LNX_X86_64_LIMA is not running"
    blocked lnx-x86_64 throughput "Lima instance $LNX_X86_64_LIMA is not running"
  fi
  "$LIMA_COURT" release lnx-x86_64-rosetta >/dev/null 2>&1 || true
  "$LIMA_COURT" lease lnx-x86_64-kernel >/dev/null 2>&1 || true
  if lima_running "$LNX_X86_64_KERNEL_LIMA"; then
    run_lima_linux_stage lnx-x86_64 x86-kernel-logic "$LNX_X86_64_KERNEL_LIMA" \
      "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" logic
  else
    blocked lnx-x86_64 x86-kernel-logic \
      "Lima instance $LNX_X86_64_KERNEL_LIMA is not running"
  fi
  "$LIMA_COURT" release lnx-x86_64-kernel >/dev/null 2>&1 || true
else
  not_requested lnx-x86_64 lima-accelerator \
    "set MINICON_ENABLE_LIMA_ACCELERATOR=1 for the optional fast court"
fi
if [ -n "${MINICON_LNX_X86_64_DESKTOP_RUNNER:-}" ] &&
   [ -x "$MINICON_LNX_X86_64_DESKTOP_RUNNER" ]; then
  run_linux_desktop_runner_stage lnx-x86_64 desktop-runtime-status \
    "$MINICON_LNX_X86_64_DESKTOP_RUNNER" \
    "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" status
  run_linux_desktop_runner_stage lnx-x86_64 desktop-test \
    "$MINICON_LNX_X86_64_DESKTOP_RUNNER" \
    "$BUILD_REL/lnx-x86_64/x86_64-unknown-linux-gnu" test
  run_stage common linux-x86_64-desktop-idle stop_linux_desktop_runner \
    lnx-x86_64 "$MINICON_LNX_X86_64_DESKTOP_RUNNER"
else
  blocked lnx-x86_64 desktop-runtime-status \
    "MINICON_LNX_X86_64_DESKTOP_RUNNER is not configured"
  blocked lnx-x86_64 desktop-test \
    "MINICON_LNX_X86_64_DESKTOP_RUNNER is not configured"
fi

if [ "$LIMA_ACCELERATOR" = 1 ]; then
  "$LIMA_COURT" lease lnx-aarch64-fast >/dev/null 2>&1 || true
  if lima_running "$LNX_AARCH64_LIMA"; then
    run_lima_linux_stage lnx-aarch64 runtime-status "$LNX_AARCH64_LIMA" \
      "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" status
    run_lima_linux_stage lnx-aarch64 test "$LNX_AARCH64_LIMA" \
      "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" test
    run_lima_linux_stage lnx-aarch64 throughput "$LNX_AARCH64_LIMA" \
      "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" throughput
  else
    blocked lnx-aarch64 runtime-status "Lima instance $LNX_AARCH64_LIMA is not running"
    blocked lnx-aarch64 test "Lima instance $LNX_AARCH64_LIMA is not running"
    blocked lnx-aarch64 throughput "Lima instance $LNX_AARCH64_LIMA is not running"
  fi
  "$LIMA_COURT" release lnx-aarch64-fast >/dev/null 2>&1 || true
else
  not_requested lnx-aarch64 lima-accelerator \
    "set MINICON_ENABLE_LIMA_ACCELERATOR=1 for the optional fast court"
fi
if [ -n "${MINICON_LNX_AARCH64_DESKTOP_RUNNER:-}" ] &&
   [ -x "$MINICON_LNX_AARCH64_DESKTOP_RUNNER" ]; then
  run_linux_desktop_runner_stage lnx-aarch64 desktop-runtime-status \
    "$MINICON_LNX_AARCH64_DESKTOP_RUNNER" \
    "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" status
  run_linux_desktop_runner_stage lnx-aarch64 desktop-test \
    "$MINICON_LNX_AARCH64_DESKTOP_RUNNER" \
    "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" test
  run_linux_desktop_runner_stage lnx-aarch64 desktop-throughput \
    "$MINICON_LNX_AARCH64_DESKTOP_RUNNER" \
    "$BUILD_REL/lnx-aarch64/aarch64-unknown-linux-gnu" throughput
  run_stage common linux-aarch64-desktop-idle stop_linux_desktop_runner \
    lnx-aarch64 "$MINICON_LNX_AARCH64_DESKTOP_RUNNER"
else
  blocked lnx-aarch64 desktop-runtime-status \
    "MINICON_LNX_AARCH64_DESKTOP_RUNNER is not configured"
  blocked lnx-aarch64 desktop-test \
    "MINICON_LNX_AARCH64_DESKTOP_RUNNER is not configured"
  blocked lnx-aarch64 desktop-throughput \
    "MINICON_LNX_AARCH64_DESKTOP_RUNNER is not configured"
fi

if [ "$LIMA_ACCELERATOR" = 1 ]; then
  lima_stop_if_running "$LNX_X86_64_LIMA"
  lima_stop_if_running "$LNX_X86_64_KERNEL_LIMA"
  lima_stop_if_running "$LNX_AARCH64_LIMA"
fi

python3 - "$RESULTS" "$RECEIPT" "$SOURCE_STATE_START" "$BUILD_DIR" <<'PY'
import csv
import datetime
import hashlib
import json
import platform
import subprocess
import sys
from pathlib import Path

results_path = Path(sys.argv[1])
receipt_path = Path(sys.argv[2])
source_state_start = json.loads(sys.argv[3])
build_dir = Path(sys.argv[4])
stages = []
with results_path.open(newline="", encoding="utf-8") as source:
    for cell, stage, status, seconds, log, detail in csv.reader(source, delimiter="\t"):
        stages.append({
            "cell": cell,
            "stage": stage,
            "status": status,
            "duration_seconds": int(seconds),
            "log": log or None,
            "detail": detail,
        })

def output(*command):
    return subprocess.check_output(command, text=True).strip()

source_state_end = json.loads(output("python3", "scripts/source-fingerprint.py"))
if source_state_end != source_state_start:
    stages.append({
        "cell": "common",
        "stage": "source-stability",
        "status": "FAIL",
        "duration_seconds": 0,
        "log": None,
        "detail": (
            f"worktree changed during qualification: "
            f"{source_state_start['sha256']} -> {source_state_end['sha256']}"
        ),
    })
else:
    stages.append({
        "cell": "common",
        "stage": "source-stability",
        "status": "PASS",
        "duration_seconds": 0,
        "log": None,
        "detail": source_state_end["sha256"],
    })

receipt = {
    "schema_version": 1,
    "product": "minicon",
    "source_sha": output("git", "rev-parse", "HEAD"),
    "source_dirty": bool(output("git", "status", "--porcelain")),
    "source_tree_sha256": source_state_end["sha256"],
    "source_file_count": source_state_end["files"],
    "build_root": build_dir.relative_to(Path.cwd()).as_posix(),
    "generated_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "host": {"system": platform.system(), "machine": platform.machine()},
    "stages": stages,
    "artifacts": [],
    "summary": {
        key: sum(stage["status"] == key for stage in stages)
        for key in ("PASS", "FAIL", "BLOCKED")
    },
}
artifact_paths = {
    "osx-aarch64": "osx-aarch64/aarch64-apple-darwin/debug/minicon",
    "osx-x86_64": "osx-x86_64/x86_64-apple-darwin/debug/minicon",
    "win-x86_64": "win-x86_64/x86_64-pc-windows-msvc/debug/minicon.exe",
    "win-aarch64": "win-aarch64/aarch64-pc-windows-msvc/debug/minicon.exe",
    "lnx-x86_64": "lnx-x86_64/x86_64-unknown-linux-gnu/debug/minicon",
    "lnx-aarch64": "lnx-aarch64/aarch64-unknown-linux-gnu/debug/minicon",
}
for cell, relative in artifact_paths.items():
    path = build_dir / relative
    if not path.is_file():
        continue
    receipt["artifacts"].append({
        "cell": cell,
        "path": path.relative_to(Path.cwd()).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "format": output("file", "-b", str(path)),
    })
receipt_path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
print(f"[six-cell] receipt: {receipt_path}")
print(json.dumps(receipt["summary"], sort_keys=True))
sys.exit(1 if receipt["summary"]["FAIL"] else 0)
PY
