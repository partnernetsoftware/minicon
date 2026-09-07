#!/bin/bash
# Execute exact tinygui artifacts. One UTM lease at a time. Missing court is
# BLOCKED, never a skipped PASS. osx-x86_64 has no UTM row: Rosetta on host.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=../../scripts/lib/utm-court.sh
. "$ROOT/scripts/lib/utm-court.sh"
COURT_CLI="$(minicon_utm_court_cli)" || exit 2
export UTM_COURT_STATE_DIR="${UTM_COURT_STATE_DIR:-$HERE/target/utm-court-service}"
DIST="$HERE/dist"
LOG="$HERE/target/court-six.log"
RECEIPT="$HERE/target/court-six-receipts.jsonl"
mkdir -p "$HERE/target"
: >"$LOG"
: >"$RECEIPT"

record() {
  printf '%s\n' "$1" | tee -a "$RECEIPT"
}

blocked() {
  local cell="$1" why="$2"
  record "{\"cell\":\"$cell\",\"status\":\"BLOCKED\",\"why\":\"$why\"}"
}

ok_line() {
  local cell="$1" line="$2"
  record "{\"cell\":\"$cell\",\"status\":\"ok\",\"guest\":$line}"
}

measure_host_unix() {
  local cell="$1" bin="$2" prefix="${3:-}"
  if [[ ! -x "$bin" ]]; then
    blocked "$cell" "missing $bin"
    return 0
  fi
  if [[ -n "$prefix" ]]; then
    "$prefix" "$HERE/guest-unix.sh" "$bin" | tee -a "$LOG"
  else
    "$HERE/guest-unix.sh" "$bin" | tee -a "$LOG"
  fi
  local line
  line="$(grep TINYGUI_RECEIPT "$LOG" | tail -n 1 | sed 's/^TINYGUI_RECEIPT //')"
  if [[ -z "$line" ]]; then
    blocked "$cell" "no TINYGUI_RECEIPT"
    return 0
  fi
  ok_line "$cell" "$line"
}

court() { "$COURT_CLI" "$@"; }

run_linux() {
  local cell="$1"
  local court_id="$2"
  local bin="$DIST/$cell/tinygui"
  if [[ ! -x "$bin" ]]; then
    blocked "$cell" "missing artifact"
    return 0
  fi
  if ! court lease "$court_id" --disposable >/dev/null; then
    blocked "$cell" "lease failed"
    return 0
  fi
  if ! court wait-ready "$court_id" 180 >/dev/null; then
    blocked "$cell" "wait-ready failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  # Same X11 court MiniCon RSS uses: QGA + xvfb, not the login session.
  local guest_dir="/tmp/tinygui-court"
  if ! court exec "$court_id" -- /bin/mkdir -p "$guest_dir"; then
    blocked "$cell" "mkdir failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  if ! court push "$court_id" "$bin" "$guest_dir/tinygui"; then
    blocked "$cell" "push binary failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  if ! court push "$court_id" "$HERE/guest-unix.sh" "$guest_dir/guest-unix.sh"; then
    blocked "$cell" "push script failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  court exec "$court_id" -- /bin/chmod +x "$guest_dir/tinygui" "$guest_dir/guest-unix.sh" || true
  set +e
  court exec "$court_id" -- /bin/bash -lc \
    "timeout 60s xvfb-run -a -s '-screen 0 1280x900x24' dbus-run-session -- /bin/bash '$guest_dir/guest-unix.sh' '$guest_dir/tinygui'" |
    tee -a "$LOG"
  local rc=${PIPESTATUS[0]}
  set -e
  court release "$court_id" >/dev/null || true
  local line
  line="$(grep TINYGUI_RECEIPT "$LOG" | tail -n 1 | sed 's/^TINYGUI_RECEIPT //')"
  if [[ "$rc" -eq 3 ]]; then
    blocked "$cell" "exec BLOCKED"
    return 0
  fi
  if [[ -z "$line" ]]; then
    blocked "$cell" "no TINYGUI_RECEIPT rc=$rc"
    return 0
  fi
  ok_line "$cell" "$line"
}

run_windows() {
  local cell="$1"
  local court_id="$2"
  local bin="$DIST/$cell/tinygui.exe"
  if [[ ! -f "$bin" ]]; then
    blocked "$cell" "missing artifact"
    return 0
  fi
  if ! court lease "$court_id" --disposable >/dev/null; then
    blocked "$cell" "lease failed"
    return 0
  fi
  if ! court wait-ready "$court_id" 120 >/dev/null; then
    blocked "$cell" "wait-ready failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  if ! court interactive-ready "$court_id" 180 >/dev/null; then
    blocked "$cell" "interactive-ready failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  local windows_root
  windows_root="$("$COURT_CLI" windows-root)"
  local guest_dir="$windows_root\\tinygui"
  if ! court exec "$court_id" -- powershell.exe -NoProfile -Command \
    "New-Item -ItemType Directory -Force -Path '$guest_dir' | Out-Null"; then
    blocked "$cell" "mkdir failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  if ! court push "$court_id" "$bin" "$guest_dir\\tinygui.exe"; then
    blocked "$cell" "push binary failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  if ! court push "$court_id" "$HERE/guest-win.ps1" "$guest_dir\\guest-win.ps1"; then
    blocked "$cell" "push script failed"
    court release "$court_id" >/dev/null || true
    return 0
  fi
  local job_id="tinygui_${cell}_$$_$RANDOM"
  local result="$windows_root\\job-$job_id.exit"
  local result_tmp="$result.tmp"
  local log="$windows_root\\job-$job_id.log"
  local job="$windows_root\\agent-v2\\job.pending.ps1"
  local ready="$windows_root\\agent-v2\\job.ready"
  printf '%s\n' \
    '$ErrorActionPreference = "Stop"' \
    "\$exitCode = 1" \
    'try {' \
    "    & '$guest_dir\\guest-win.ps1' -Bin '$guest_dir\\tinygui.exe' *> '$log'" \
    '    $exitCode = 0' \
    '} catch {' \
    "    \$_ | Out-String | Add-Content -LiteralPath '$log'" \
    '    $exitCode = 1' \
    '} finally {' \
    "    [IO.File]::WriteAllText('$result_tmp', [string]\$exitCode)" \
    "    Move-Item -LiteralPath '$result_tmp' -Destination '$result' -Force" \
    '}' \
    'exit $exitCode' | court push "$court_id" - "$job"
  printf 'ready' | court push "$court_id" - "$ready"
  local scratch
  scratch="$(mktemp -d)"
  local deadline=$((SECONDS + 600))
  set +e
  while :; do
    : >"$scratch/exit"
    court pull "$court_id" "$result" "$scratch/exit" 2>/dev/null || true
    [[ -s "$scratch/exit" ]] && break
    if [[ "$SECONDS" -ge "$deadline" ]]; then
      blocked "$cell" "windows job deadline"
      court release "$court_id" >/dev/null || true
      rm -rf "$scratch"
      set -e
      return 0
    fi
    sleep 1
  done
  court pull "$court_id" "$log" - 2>/dev/null | tee -a "$LOG"
  set -e
  court release "$court_id" >/dev/null || true
  rm -rf "$scratch"
  local line
  line="$(grep TINYGUI_RECEIPT "$LOG" | tail -n 1 | sed 's/^TINYGUI_RECEIPT //')"
  if [[ -z "$line" ]]; then
    blocked "$cell" "no TINYGUI_RECEIPT"
    return 0
  fi
  ok_line "$cell" "$line"
}

echo "[tinygui-court] $COURT_CLI"
measure_host_unix osx-aarch64-host "$DIST/osx-aarch64/tinygui"
# No OSX x86_64 UTM row. The x86_64 Mach-O runs under Rosetta on this host.
measure_host_unix osx-x86_64-rosetta "$DIST/osx-x86_64/tinygui"

run_linux lnx-aarch64 lnx-aarch64-desktop
run_linux lnx-x86_64 lnx-x86_64-desktop
run_windows win-aarch64 win-aarch64-desktop
run_windows win-x86_64 win-x86_64-desktop

echo "[tinygui-court] receipts $RECEIPT"
cat "$RECEIPT"
