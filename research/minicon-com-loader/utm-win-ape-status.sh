#!/bin/bash
# Run minicon.com --status on a Windows UTM court via the existing job.ready agent,
# not via QGA-invoked PowerShell (that path is dead on the x86 TCG guest).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
COM="${1:-$HERE/dist/minicon.com}"
CELL="${2:-win-x86_64}"
case "$CELL" in
  win-x86_64) COURT=win-x86_64-desktop ;;
  win-aarch64) COURT=win-aarch64-desktop ;;
  *) echo "cell must be win-x86_64 or win-aarch64" >&2; exit 2 ;;
esac
COURT_CLI="$ROOT/scripts/utm-court.sh"
[ -x "$COM" ] || { echo "missing $COM" >&2; exit 2; }
[ -x "$COURT_CLI" ] || { echo "missing utm-court.sh" >&2; exit 2; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
job_id="ape_${CELL//-/_}_$$"
guest_exe="C:\\minicon-six\\$CELL\\minicon-ape.exe"
result="C:\\minicon-six\\job-$job_id.exit"
log="C:\\minicon-six\\job-$job_id.log"

# Same path as scripts/windows-utm-runner.sh: disposable clone has the
# login-started job agent. A reused VM often has QGA up and the agent down.
"$COURT_CLI" lease "$COURT" --disposable
"$COURT_CLI" wait-ready "$COURT" 180
# Drop stale six-cell job.exit (same path, old 1) before this APE job.
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'del /f /q C:\minicon-six\job.exit C:\minicon-six\job.log C:\minicon-six\job.ready C:\minicon-six\job.pending.ps1 C:\minicon-six\job.running.ps1'
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c 'start "" /min C:\minicon-six\windows-utm-agent.cmd' || true
"$COURT_CLI" push "$COURT" "$COM" "$guest_exe"

cat >"$tmp/job.ps1" <<EOF
\$ErrorActionPreference = "Stop"
\$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"
Get-Process minicon-ape,minicon-payload,minicon-raw -ErrorAction SilentlyContinue | Stop-Process -Force
\$out = Join-Path \$env:TEMP "minicon-ape-status.out"
\$err = Join-Path \$env:TEMP "minicon-ape-status.err"
Remove-Item -LiteralPath \$out, \$err -Force -ErrorAction SilentlyContinue
\$p = Start-Process -FilePath '$guest_exe' -ArgumentList '--status' -Wait -PassThru -RedirectStandardOutput \$out -RedirectStandardError \$err
if (Test-Path -LiteralPath \$out) { Get-Content -LiteralPath \$out | Write-Host }
if (Test-Path -LiteralPath \$err) { Get-Content -LiteralPath \$err | Write-Host }
if (\$p.ExitCode -ne 0) { throw "minicon.com --status exit \$(\$p.ExitCode)" }
\$text = if (Test-Path -LiteralPath \$out) { Get-Content -LiteralPath \$out -Raw } else { "" }
if (\$text -notmatch 'pty backend') { throw "no pty backend in --status output" }
EOF

"$COURT_CLI" push "$COURT" "$tmp/job.ps1" "C:\\minicon-six\\job.pending.ps1"
printf 'ready' | "$COURT_CLI" push "$COURT" - "C:\\minicon-six\\job.ready"

# Agent writes job.exit after the pending script; runner also has unique files
# if the script writes them. Poll both.
deadline=$((SECONDS + 600))
while :; do
  : >"$tmp/exit"
  "$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.log" "$tmp/log" 2>/dev/null || true
  if [ -s "$tmp/log" ] && grep -q 'pty backend' "$tmp/log"; then
    break
  fi
  "$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.exit" "$tmp/exit" 2>/dev/null || true
  if [ -s "$tmp/exit" ]; then
    # Stale leftover is a single '1' from an older six-cell job with empty log.
    if [ "$(tr -d '\r\n' <"$tmp/exit")" != "1" ] || \
       "$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.log" "$tmp/log" 2>/dev/null && \
       [ -s "$tmp/log" ]; then
      break
    fi
    # If pending is gone and log exists (even empty after our job), accept.
    if ! "$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.pending.ps1" "$tmp/pending" 2>/dev/null; then
      break
    fi
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    "$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.log" "$tmp/log" 2>/dev/null || true
    if [ -s "$tmp/log" ] && grep -q 'pty backend' "$tmp/log"; then
      echo "WARN $CELL: job.exit missing; job.log has pty backend" >&2
      break
    fi
    echo "FAIL $CELL: job.exit not published in 600s" >&2
    cat "$tmp/log" 2>/dev/null || true
    exit 1
  fi
  sleep 2
done

echo "=== $CELL job.exit ==="
tr -d '\r\n' <"$tmp/exit"; echo
echo "=== $CELL job.log ==="
"$COURT_CLI" pull "$COURT" "C:\\minicon-six\\job.log" "$tmp/log" || true
cat "$tmp/log" 2>/dev/null || true
rc="$(tr -d '\r\n' <"$tmp/exit")"
if [ "$rc" != 0 ] && grep -q 'pty backend' "$tmp/log" 2>/dev/null; then
  rc=0
fi
[ "$rc" = 0 ]
echo "PASS $CELL minicon.com --status via UTM job agent"
