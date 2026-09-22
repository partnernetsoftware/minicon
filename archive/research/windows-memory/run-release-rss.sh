#!/bin/bash
# Research-only: RSS black box against a release Windows PE.
# Does not modify scripts/windows-utm-runner.sh. Does not overwrite hang-diag/.

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
PIN="$(python3 - "$REPO_ROOT/Cargo.toml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
m = re.search(r'rev = "([0-9a-f]{40})"', text)
print(m.group(1) if m else "unknown")
PY
)"
PE_SHA="$(shasum -a 256 "$TARGET_TRIPLE/minicon.exe" | awk '{print $1}')"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG="$LOG_DIR/rss-win-aarch64-release-$SOURCE-$STAMP.log"

ts() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*"; }

[ -f "$TARGET_TRIPLE/minicon.exe" ] || {
  echo "missing $TARGET_TRIPLE/minicon.exe" >&2
  exit 2
}

harness=""
for candidate in "$TARGET_TRIPLE/deps"/minicon_control-*.exe; do
  [ -f "$candidate" ] || continue
  case "${candidate##*/}" in
    minicon_control-[0-9a-f]*.exe) harness="$candidate" ;;
  esac
done
[ -n "$harness" ] && [ -f "$harness" ] || {
  echo "missing minicon_control harness under $TARGET_TRIPLE/deps" >&2
  exit 2
}

{
  ts "source=$SOURCE"
  ts "pin=$PIN"
  ts "pe_sha256=$PE_SHA"
  ts "pe_path=$TARGET_TRIPLE/minicon.exe"
  ts "harness=$harness"
  ts "note=33,620K tasklist WS is hang evidence only; not a public idle court"
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
GUEST_TEST="$GUEST_ROOT\\target\\debug\\minicon-control-release-rss.exe"
ts "push product+harness" | tee -a "$LOG"
court push "$COURT" "$TARGET_TRIPLE/minicon.exe" "$GUEST_EXE"
court push "$COURT" "$harness" "$GUEST_TEST"

runner_tmp="$(mktemp -d)"
trap 'rm -rf "$runner_tmp"; cleanup' EXIT
job_id="win_aarch64_rss_release_$$_$RANDOM"
RESULT="$WINDOWS_ROOT\\job-$job_id.exit"
RESULT_TMP="$RESULT.tmp"
GUEST_LOG="$WINDOWS_ROOT\\job-$job_id.log"
JOB="$WINDOWS_ROOT\\agent-v2\\job.pending.ps1"
READY="$WINDOWS_ROOT\\agent-v2\\job.ready"
ts "job_id=$job_id" | tee -a "$LOG"

# Never Start-Process -Wait (waits for all descendants). Open Handle on the
# PassThru object, WaitForExit(ms) on that PID only, then Refresh. Null
# ExitCode is wrapper failure, never coerced to 0. Raw fields go to .exitmeta.
printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  '$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"' \
  '$env:AGENTERM_NO_ACTIVATE = "1"' \
  "\$env:MINICON_TEST_BINARY = '$GUEST_EXE'" \
  "\$log = '$GUEST_LOG'" \
  "\$err = '$GUEST_LOG.err'" \
  "\$pidsFile = '$GUEST_LOG.pids'" \
  "\$metaFile = '$GUEST_LOG.exitmeta'" \
  '$names = @("minicon-release-rss","minicon-control-release-rss")' \
  '$before = @(Get-Process -Name $names -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })' \
  '$exitCode = 1' \
  'function Save-PidState([string]$phase, $harness) {' \
  '    $rows = @("phase=$phase", "harness_pid=$($harness.Id)", "harness_has_exited=$($harness.HasExited)")' \
  '    Get-Process -Name $names -ErrorAction SilentlyContinue | ForEach-Object { $rows += ("proc name=$($_.Name) pid=$($_.Id) ws=$($_.WorkingSet64) parent=$($_.Parent.Id)") }' \
  '    $rows | Set-Content -LiteralPath $pidsFile -Encoding utf8' \
  '}' \
  'function Save-ExitMeta([string]$phase, $harness) {' \
  '    $raw = $harness.ExitCode' \
  '    if ($null -eq $raw) { $rawText = "NULL" } else { $rawText = [string]$raw }' \
  '    $handleText = "unopened"' \
  '    try { $handleText = [string]$harness.Handle } catch { $handleText = $_.Exception.Message }' \
  '    @( "phase=$phase", "pid=$($harness.Id)", "HasExited=$($harness.HasExited)", "ExitCode_raw=$rawText", "Handle=$handleText" ) | Set-Content -LiteralPath $metaFile -Encoding utf8' \
  '}' \
  'try {' \
  "    \$p = Start-Process -FilePath '$GUEST_TEST' -ArgumentList '--exact','--nocapture','--test-threads=1','host_process_rss_stays_within_named_budget' -PassThru -NoNewWindow -RedirectStandardOutput \$log -RedirectStandardError \$err" \
  '    $null = $p.Handle' \
  '    Save-PidState started $p' \
  '    if (-not $p.WaitForExit(180000)) {' \
  '        Save-PidState timeout $p' \
  '        $ours = @(Get-Process -Name $names -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object { $_.Id })' \
  '        foreach ($id in $ours) { Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }' \
  '        if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }' \
  '        throw ("rss harness-pid WaitForExit timeout 180s killed pids " + ($ours -join ","))' \
  '    }' \
  '    $p.Refresh()' \
  '    Save-PidState exited $p' \
  '    Save-ExitMeta after_wait $p' \
  '    if ($null -eq $p.ExitCode) { throw "wrapper failure: ExitCode null after WaitForExit HasExited=$($p.HasExited) pid=$($p.Id)" }' \
  '    if ($p.ExitCode -ne 0) { throw "rss court exit $($p.ExitCode)" }' \
  '    $exitCode = $p.ExitCode' \
  '} catch {' \
  "    \$_ | Out-String | Add-Content -LiteralPath \$log" \
  '    $exitCode = 1' \
  '} finally {' \
  "    [IO.File]::WriteAllText('$RESULT_TMP', [string]\$exitCode)" \
  "    Move-Item -LiteralPath '$RESULT_TMP' -Destination '$RESULT' -Force" \
  '}' \
  'exit $exitCode' | court push "$COURT" - "$JOB"
printf 'ready' | court push "$COURT" - "$READY"
ts "job submitted; poll exit (host bound 210s)" | tee -a "$LOG"

deadline="$((SECONDS + 210))"
while :; do
  : >"$runner_tmp/exit"
  court pull "$COURT" "$RESULT" "$runner_tmp/exit" 2>/dev/null || true
  [ -s "$runner_tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    ts "host poll exceeded 300s waiting for job.exit" | tee -a "$LOG"
    exit 1
  fi
  sleep 1
done
court pull "$COURT" "$GUEST_LOG" "$LOG.hostout" 2>/dev/null || true
court pull "$COURT" "$GUEST_LOG.err" "$LOG.hosterr" 2>/dev/null || true
court pull "$COURT" "$GUEST_LOG.exitmeta" "$LOG.exitmeta" 2>/dev/null || true
court pull "$COURT" "$GUEST_LOG.pids" "$LOG.pids" 2>/dev/null || true
if [ -f "$LOG.hostout" ]; then cat "$LOG.hostout" | tee -a "$LOG"; fi
if [ -f "$LOG.hosterr" ]; then cat "$LOG.hosterr" | tee -a "$LOG"; fi
if [ -f "$LOG.exitmeta" ]; then echo '==== exitmeta ===='; cat "$LOG.exitmeta" | tee -a "$LOG"; fi
if [ -f "$LOG.pids" ]; then echo '==== pids ===='; cat "$LOG.pids" | tee -a "$LOG"; fi
runner_rc="$(tr -d '\r\n' <"$runner_tmp/exit")"
ts "WIN_RELEASE_RSS_EXIT=$runner_rc" | tee -a "$LOG"
echo "WIN_RELEASE_RSS_EXIT=$runner_rc" | tee -a "$LOG"
exit "$runner_rc"
