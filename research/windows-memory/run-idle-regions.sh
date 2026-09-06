#!/bin/bash
# Research-only: idle VirtualQueryEx image/mapped/private split.
# Not an RSS wrapper court. Does not cut PTY capacity.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=../../scripts/lib/utm-court.sh
. "$REPO_ROOT/scripts/lib/utm-court.sh"
COURT_CLI="$(minicon_utm_court_cli)" || exit 2
WINDOWS_ROOT="${UTM_COURT_WINDOWS_ROOT:-$("$COURT_CLI" windows-root)}"

CELL=win-aarch64
COURT=win-aarch64-desktop
VM="${MINICON_WINDOWS_UTM_AARCH64_VM:-minicon-win-arm-64}"
TARGET_TRIPLE="$REPO_ROOT/target/windows-memory/aarch64-pc-windows-msvc/release"
LOG_DIR="$REPO_ROOT/target/windows-memory"
mkdir -p "$LOG_DIR"
SOURCE="$(cd "$REPO_ROOT" && git rev-parse HEAD)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG="$LOG_DIR/idle-regions-$SOURCE-$STAMP.log"

ts() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*"; }

[ -f "$TARGET_TRIPLE/minicon.exe" ] || {
  echo "missing $TARGET_TRIPLE/minicon.exe" >&2
  exit 2
}

{
  ts "source=$SOURCE"
  ts "note=idle region split; not RSS wrapper; not macOS WritingTools"
} | tee "$LOG"

court() { UTM_COURT_VM="$VM" "$COURT_CLI" "$@"; }
open -a UTM >/dev/null 2>&1 || true

ts "lease" | tee -a "$LOG"
if [ "${MINICON_WINDOWS_UTM_DISPOSABLE:-1}" = 1 ]; then
  court lease "$COURT" --disposable >/dev/null
else
  court lease "$COURT" >/dev/null
fi
cleanup() { ts "release" | tee -a "$LOG"; court release "$COURT" >/dev/null || true; }
trap cleanup EXIT

ts "wait-ready" | tee -a "$LOG"
court wait-ready "$COURT" 120 >/dev/null
ts "interactive-ready" | tee -a "$LOG"
court interactive-ready "$COURT" 180 >/dev/null

GUEST_ROOT="$WINDOWS_ROOT\\$CELL"
GUEST_EXE="$GUEST_ROOT\\target\\debug\\minicon-release-rss.exe"
GUEST_OWNERS="$WINDOWS_ROOT\\probe-gdi-objects.ps1"
GUEST_REGIONS="$WINDOWS_ROOT\\probe-ws-regions.ps1"
GUEST_SAMPLE="$WINDOWS_ROOT\\sample-idle-regions.ps1"
ts "push product+probes" | tee -a "$LOG"
court push "$COURT" "$TARGET_TRIPLE/minicon.exe" "$GUEST_EXE"
court push "$COURT" "$SCRIPT_DIR/probe-gdi-objects.ps1" "$GUEST_OWNERS"
court push "$COURT" "$SCRIPT_DIR/probe-ws-regions.ps1" "$GUEST_REGIONS"
court push "$COURT" "$SCRIPT_DIR/sample-idle-regions.ps1" "$GUEST_SAMPLE"

runner_tmp="$(mktemp -d)"
trap 'rm -rf "$runner_tmp"; cleanup' EXIT
job_id="win_aarch64_idle_regions_$$_$RANDOM"
RESULT="$WINDOWS_ROOT\\job-$job_id.exit"
GUEST_LOG="$WINDOWS_ROOT\\job-$job_id.log"
JOB="$WINDOWS_ROOT\\agent-v2\\job.pending.ps1"
READY="$WINDOWS_ROOT\\agent-v2\\job.ready"
ts "job_id=$job_id" | tee -a "$LOG"

printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  "\$env:AGENTERM_NO_ACTIVATE = '1'" \
  "& '$GUEST_SAMPLE' -Exe '$GUEST_EXE' -OwnersProbe '$GUEST_OWNERS' -RegionsProbe '$GUEST_REGIONS' -Log '$GUEST_LOG' -Result '$RESULT'" \
  'exit $LASTEXITCODE' | court push "$COURT" - "$JOB"
printf 'ready' | court push "$COURT" - "$READY"
ts "job submitted; poll exit (host bound 90s)" | tee -a "$LOG"

deadline="$((SECONDS + 90))"
while :; do
  : >"$runner_tmp/exit"
  court pull "$COURT" "$RESULT" "$runner_tmp/exit" 2>/dev/null || true
  [ -s "$runner_tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    ts "host poll exceeded waiting for job.exit" | tee -a "$LOG"
    court pull "$COURT" "$GUEST_LOG" "$LOG.hostout" 2>/dev/null || true
    if [ -f "$LOG.hostout" ]; then cat "$LOG.hostout" | tee -a "$LOG"; fi
    exit 1
  fi
  sleep 1
done
court pull "$COURT" "$GUEST_LOG" "$LOG.hostout" 2>/dev/null || true
if [ -f "$LOG.hostout" ]; then cat "$LOG.hostout" | tee -a "$LOG"; fi
runner_rc="$(tr -d '\r\n' <"$runner_tmp/exit")"
ts "WIN_IDLE_REGIONS_EXIT=$runner_rc" | tee -a "$LOG"
echo "WIN_IDLE_REGIONS_EXIT=$runner_rc" | tee -a "$LOG"
exit "$runner_rc"
