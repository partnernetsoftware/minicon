#!/bin/bash
# Exact production PE: no-activate must not steal foreground; then English SendInput.
# No research skip env. IME on. No screenshot. Chinese compose BLOCKED without zh layout.

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
PE_DIR="$REPO_ROOT/target/windows-memory/exact-pe/aarch64-pc-windows-msvc/release"
LOG_DIR="$REPO_ROOT/target/windows-memory"
mkdir -p "$LOG_DIR"
SOURCE="$(cd "$REPO_ROOT" && git rev-parse HEAD)"
PIN="$(python3 - "$REPO_ROOT/Cargo.toml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r'rev = "([0-9a-f]{40})"', text)
print(m.group(1) if m else "unknown")
PY
)"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG="$LOG_DIR/no-activate-behavior-$SOURCE-$STAMP.log"

ts() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*"; }

[ -f "$PE_DIR/minicon.exe" ] || {
  echo "missing exact PE $PE_DIR/minicon.exe" >&2
  exit 2
}
PE_SHA="$(shasum -a 256 "$PE_DIR/minicon.exe" | awk '{print $1}')"
PE_BYTES="$(wc -c <"$PE_DIR/minicon.exe" | tr -d ' ')"

{
  ts "source=$SOURCE"
  ts "contains_fix=56207cb honor no-activate startup focus"
  ts "pin=$PIN"
  ts "exact_pe=$PE_DIR/minicon.exe"
  ts "exact_pe_sha256=$PE_SHA"
  ts "exact_pe_bytes=$PE_BYTES"
  ts "note=production source PE; no research skip vars; no pin change"
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

GUEST_EXE="$WINDOWS_ROOT\\$CELL\\target\\debug\\minicon-exact-noact.exe"
GUEST_SAMPLE="$WINDOWS_ROOT\\sample-no-activate-behavior.ps1"
ts "push exact PE" | tee -a "$LOG"
court push "$COURT" "$PE_DIR/minicon.exe" "$GUEST_EXE"
court push "$COURT" "$SCRIPT_DIR/sample-no-activate-behavior.ps1" "$GUEST_SAMPLE"

runner_tmp="$(mktemp -d)"
trap 'rm -rf "$runner_tmp"; cleanup' EXIT
job_id="win_aarch64_noact_$$_$RANDOM"
RESULT="$WINDOWS_ROOT\\job-$job_id.exit"
GUEST_LOG="$WINDOWS_ROOT\\job-$job_id.log"
JOB="$WINDOWS_ROOT\\agent-v2\\job.pending.ps1"
READY="$WINDOWS_ROOT\\agent-v2\\job.ready"
ts "job_id=$job_id" | tee -a "$LOG"

printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  "& '$GUEST_SAMPLE' -Exe '$GUEST_EXE' -Log '$GUEST_LOG' -Result '$RESULT'" \
  'exit $LASTEXITCODE' | court push "$COURT" - "$JOB"
printf 'ready' | court push "$COURT" - "$READY"
ts "job submitted; poll exit (host bound 180s)" | tee -a "$LOG"

deadline="$((SECONDS + 180))"
while :; do
  : >"$runner_tmp/exit"
  court pull "$COURT" "$RESULT" "$runner_tmp/exit" 2>/dev/null || true
  [ -s "$runner_tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    ts "host poll exceeded" | tee -a "$LOG"
    court pull "$COURT" "$GUEST_LOG" "$LOG.hostout" 2>/dev/null || true
    exit 1
  fi
  sleep 1
done
court pull "$COURT" "$GUEST_LOG" "$LOG.hostout" 2>/dev/null || true
if [ -f "$LOG.hostout" ]; then cat "$LOG.hostout" | tee -a "$LOG"; fi
runner_rc="$(tr -d '\r\n' <"$runner_tmp/exit")"
ts "WIN_NOACT_BEHAVIOR_EXIT=$runner_rc" | tee -a "$LOG"
echo "WIN_NOACT_BEHAVIOR_EXIT=$runner_rc" | tee -a "$LOG"
exit "$runner_rc"
