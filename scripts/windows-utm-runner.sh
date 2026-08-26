#!/bin/bash
# Bridge six-cell-qualify.sh to a Windows VM managed by UTM.
# The VM must have UTM Windows Guest Tools (QEMU guest agent) installed and an
# auto-login interactive desktop for the public GUI journeys. A stopped or
# suspended VM is started here; routine cold starts use UTM's disposable mode.

set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: scripts/windows-utm-runner.sh CELL TARGET_DIR status|test|throughput|stop" >&2
  exit 2
fi

CELL="$1"
TARGET_DIR="$2"
MODE="$3"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
UTMCTL="${MINICON_UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"

case "$CELL" in
  win-aarch64)
    VM="${MINICON_WINDOWS_UTM_AARCH64_VM:-minicon-win-arm64}"
    ;;
  win-x86_64)
    VM="${MINICON_WINDOWS_UTM_X86_64_VM:-${MINICON_WINDOWS_UTM_AARCH64_VM:-minicon-win-arm64}}"
    ;;
  *)
    echo "unsupported Windows cell: $CELL" >&2
    exit 2
    ;;
esac

case "$MODE" in
  status|test|throughput|stop) ;;
  *)
    echo "unsupported Windows runner mode: $MODE" >&2
    exit 2
    ;;
esac

[ -x "$UTMCTL" ] || {
  echo "utmctl not found: $UTMCTL" >&2
  exit 2
}

if [ "$MODE" = stop ]; then
  vm_status="$($UTMCTL status "$VM" 2>/dev/null || true)"
  if [ "$vm_status" != stopped ]; then
    "$UTMCTL" stop "$VM" >/dev/null 2>&1 || true
    for _ in $(seq 1 90); do
      [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = stopped ] && exit 0
      sleep 1
    done
    echo "UTM VM did not reach stopped state within 90 seconds: $VM" >&2
    exit 1
  fi
  exit 0
fi

runner_tmp="$(mktemp -d)"
trap 'rm -rf "$runner_tmp"' EXIT

vm_status="$($UTMCTL status "$VM" 2>/dev/null || true)"
case "$vm_status" in
  started)
    ;;
  stopped)
    if [ "${MINICON_WINDOWS_UTM_DISPOSABLE:-1}" = 1 ]; then
      "$UTMCTL" start --hide --disposable "$VM" >/dev/null 2>&1
    else
      "$UTMCTL" start --hide "$VM" >/dev/null 2>&1
    fi
    ;;
  suspended)
    "$UTMCTL" start --hide "$VM" >/dev/null 2>&1
    ;;
  *)
    echo "cannot start UTM VM '$VM' from status '${vm_status:-unknown}'" >&2
    exit 1
    ;;
esac

# A running VM can expose networking and the desktop before QEMU Guest Agent
# finishes service startup. utmctl may report an OSStatus error yet still exit
# zero, so an exit-code-only probe is not evidence. Require a nonce to survive
# a complete host -> guest -> host round trip before transferring artifacts.
guest_ready=0
ready_token="minicon-$CELL-$$-$RANDOM"
for _ in $(seq 1 120); do
  : >"$runner_tmp/guest-agent.ready"
  printf '%s' "$ready_token" | "$UTMCTL" file push "$VM" \
    'C:\minicon-six\guest-agent.ready' >/dev/null 2>&1 || true
  "$UTMCTL" file pull "$VM" 'C:\minicon-six\guest-agent.ready' \
    >"$runner_tmp/guest-agent.ready" 2>/dev/null || true
  if [ "$(cat "$runner_tmp/guest-agent.ready")" = "$ready_token" ]; then
    guest_ready=1
    break
  fi
  sleep 1
done
[ "$guest_ready" -eq 1 ] || {
  echo "UTM guest agent did not become ready within 120 seconds: $VM" >&2
  exit 1
}

HOST_TARGET="$REPO_ROOT/$TARGET_DIR"
if [ "$MODE" = throughput ]; then
  PROFILE="release-fast"
else
  PROFILE="debug"
fi
HOST_PROFILE="$HOST_TARGET/$PROFILE"
[ -f "$HOST_PROFILE/minicon.exe" ] || {
  echo "Windows artifact missing: $HOST_PROFILE/minicon.exe" >&2
  exit 2
}

build_identity="$(printf '%s\n' "$TARGET_DIR" | sed -n 's#.*target-six/builds/\([0-9a-f]\{64\}\)/.*#\1#p')"
[ -n "$build_identity" ] || {
  echo "Windows target directory lacks a source-fingerprint identity: $TARGET_DIR" >&2
  exit 2
}
GUEST_ROOT="C:\\minicon-six\\$CELL"
"$UTMCTL" file push "$VM" "$GUEST_ROOT\\windows-runtime-qualify.ps1" \
  <"$SCRIPT_DIR/windows-runtime-qualify.ps1"

GUEST_DEPS="$GUEST_ROOT\\target\\debug\\deps"
guest_product="minicon-$build_identity-$PROFILE.exe"
"$UTMCTL" file push "$VM" "$GUEST_ROOT\\target\\debug\\$guest_product" \
  <"$HOST_PROFILE/minicon.exe"

python3 - "$HOST_PROFILE/deps" "$build_identity" "$PROFILE" \
    >"$runner_tmp/test-manifest.json" <<'PY'
import json
import re
import sys
from pathlib import Path

deps = Path(sys.argv[1])
identity = sys.argv[2]
profile = sys.argv[3]
prefixes = (
    "minicon",
    "minicon_core",
    "minicon_alignment",
    "minicon_load_portability",
    "minicon_console_agent",
    "minicon_control",
    "minicon_blackbox",
    "minicon_throughput",
)
tests = {}
for prefix in prefixes:
    pattern = re.compile(rf"^{re.escape(prefix)}-[0-9a-f]+\.exe$")
    matches = sorted(path.name for path in deps.glob(f"{prefix}-*.exe") if pattern.fullmatch(path.name))
    if len(matches) != 1:
        raise SystemExit(f"expected exactly one {prefix} harness in {deps}, found {matches}")
    tests[prefix] = f"{identity}-{profile}-{matches[0]}"
print(json.dumps({
    "source_tree_sha256": identity,
    "profile": profile,
    "product": f"minicon-{identity}-{profile}.exe",
    "tests": tests,
}))
PY
while IFS=$'\t' read -r host_name guest_name; do
  "$UTMCTL" file push "$VM" "$GUEST_DEPS\\$guest_name" \
    <"$HOST_PROFILE/deps/$host_name"
done < <(python3 - "$runner_tmp/test-manifest.json" <<'PY'
import json
import sys

manifest = json.load(open(sys.argv[1]))
identity = manifest["source_tree_sha256"]
profile = manifest["profile"]
for guest_name in manifest["tests"].values():
    print(f"{guest_name[len(identity) + len(profile) + 2:]}\t{guest_name}")
PY
)
"$UTMCTL" file push "$VM" "$GUEST_ROOT\\target\\test-manifest.json" \
  <"$runner_tmp/test-manifest.json"

JOB="C:\\minicon-six\\job.pending.ps1"
READY="C:\\minicon-six\\job.ready"
job_id="${CELL//-/_}_${MODE}_$$_${RANDOM}"
RESULT="C:\\minicon-six\\job-$job_id.exit"
RESULT_TMP="$RESULT.tmp"
LOG="C:\\minicon-six\\job-$job_id.log"

printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  '$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"' \
  '$exitCode = 1' \
  'try {' \
  "    & powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '$GUEST_ROOT\\windows-runtime-qualify.ps1' -TargetDir '$GUEST_ROOT\\target' -Mode '$MODE' *> '$LOG'" \
  '    $exitCode = $LASTEXITCODE' \
  '    if ($null -eq $exitCode) { $exitCode = 1 }' \
  '} catch {' \
  "    \$_ | Out-String | Add-Content -LiteralPath '$LOG'" \
  '    $exitCode = 1' \
  '} finally {' \
  "    [IO.File]::WriteAllText('$RESULT_TMP', [string]\$exitCode)" \
  "    Move-Item -LiteralPath '$RESULT_TMP' -Destination '$RESULT' -Force" \
  '}' \
  'exit $exitCode' | "$UTMCTL" file push "$VM" "$JOB"
printf 'ready' | "$UTMCTL" file push "$VM" "$READY"

# Each job publishes a unique result path atomically, so no prior run can be
# mistaken for current-source evidence.
deadline="$((SECONDS + 1200))"
while :; do
  : >"$runner_tmp/exit"
  "$UTMCTL" file pull "$VM" "$RESULT" >"$runner_tmp/exit" 2>/dev/null || true
  [ -s "$runner_tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "interactive Windows test job exceeded its 20-minute deadline" >&2
    exit 1
  fi
  sleep 1
done
"$UTMCTL" file pull "$VM" "$LOG" || true
runner_rc="$(tr -d '\r\n' <"$runner_tmp/exit")"
case "$runner_rc" in
  ''|*[!0-9]*)
    echo "invalid Windows test result: $runner_rc" >&2
    exit 1
    ;;
esac
exit "$runner_rc"
