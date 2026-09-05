#!/bin/bash
# Bridge six-cell-qualify.sh to a Windows VM managed by UTM.
# The VM must have UTM Windows Guest Tools (QEMU guest agent) installed and an
# auto-login interactive desktop for the public GUI journeys. A stopped or
# suspended VM is started here; routine cold starts use UTM's disposable mode.

set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: scripts/windows-utm-runner.sh CELL TARGET_DIR status|test|throughput|console-agent|stop" >&2
  exit 2
fi

CELL="$1"
TARGET_DIR="$2"
MODE="$3"
CONSOLE_AGENT_FILTER="${MINICON_WINDOWS_CONSOLE_AGENT_FILTER:-}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib/utm-court.sh
. "$SCRIPT_DIR/lib/utm-court.sh"
COURT_CLI="$(minicon_utm_court_cli)" || exit 2
WINDOWS_ROOT="${UTM_COURT_WINDOWS_ROOT:-$("$COURT_CLI" windows-root)}"

case "$CELL" in
  win-aarch64)
    VM="${MINICON_WINDOWS_UTM_AARCH64_VM:-minicon-win-arm-64}"
    ;;
  win-x86_64)
    VM="${MINICON_WINDOWS_UTM_X86_64_VM:-minicon-win-x86-64}"
    ;;
  *)
    echo "unsupported Windows cell: $CELL" >&2
    exit 2
    ;;
esac

case "$MODE" in
  status|test|throughput|console-agent|stop) ;;
  *)
    echo "unsupported Windows runner mode: $MODE" >&2
    exit 2
    ;;
esac
if [ -n "$CONSOLE_AGENT_FILTER" ] &&
   ! [[ "$CONSOLE_AGENT_FILTER" =~ ^[A-Za-z0-9_]+$ ]]; then
  echo "invalid console-agent test filter: $CONSOLE_AGENT_FILTER" >&2
  exit 2
fi

case "$CELL" in
  win-aarch64) COURT=win-aarch64-desktop ;;
  win-x86_64) COURT=win-x86_64-desktop ;;
esac
court() { UTM_COURT_VM="$VM" "$COURT_CLI" "$@"; }

if [ "$MODE" = stop ]; then
  court release "$COURT" >/dev/null
  exit $?
fi

runner_tmp="$(mktemp -d)"
trap 'rm -rf "$runner_tmp"' EXIT

if [ "${MINICON_WINDOWS_UTM_DISPOSABLE:-1}" = 1 ]; then
  court lease "$COURT" --disposable >/dev/null
else
  court lease "$COURT" >/dev/null
fi

# Product-neutral court automation owns Guest Agent readiness. MiniCon begins
# only after that shared adapter reports a typed ready state.
court wait-ready "$COURT" 120 >/dev/null
court interactive-ready "$COURT" 180 >/dev/null

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
GUEST_ROOT="$WINDOWS_ROOT\\$CELL"
court push "$COURT" "$SCRIPT_DIR/windows-runtime-qualify.ps1" \
  "$GUEST_ROOT\\windows-runtime-qualify.ps1"

GUEST_DEPS="$GUEST_ROOT\\target\\debug\\deps"
guest_product="minicon-$build_identity-$PROFILE.exe"
court push "$COURT" "$HOST_PROFILE/minicon.exe" \
  "$GUEST_ROOT\\target\\debug\\$guest_product"

python3 - "$HOST_PROFILE/deps" "$build_identity" "$PROFILE" "$MODE" \
    >"$runner_tmp/test-manifest.json" <<'PY'
import json
import re
import sys
from pathlib import Path

deps = Path(sys.argv[1])
identity = sys.argv[2]
profile = sys.argv[3]
mode = sys.argv[4]
prefixes = {
    "status": (),
    "console-agent": ("minicon_console_agent",),
    "test": (
        "minicon",
        "minicon_core",
        "minicon_load_portability",
        "minicon_console_agent",
        "minicon_control",
        "minicon_blackbox",
    ),
    "throughput": ("minicon_throughput",),
}[mode]
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
  court push "$COURT" "$HOST_PROFILE/deps/$host_name" \
    "$GUEST_DEPS\\$guest_name"
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
court push "$COURT" "$runner_tmp/test-manifest.json" \
  "$GUEST_ROOT\\target\\test-manifest.json"

JOB="$WINDOWS_ROOT\\agent-v2\\job.pending.ps1"
READY="$WINDOWS_ROOT\\agent-v2\\job.ready"
job_id="${CELL//-/_}_${MODE}_$$_${RANDOM}"
RESULT="$WINDOWS_ROOT\\job-$job_id.exit"
RESULT_TMP="$RESULT.tmp"
LOG="$WINDOWS_ROOT\\job-$job_id.log"

printf '%s\n' \
  '$ErrorActionPreference = "Stop"' \
  '$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"' \
  "\$env:MINICON_WINDOWS_CONSOLE_AGENT_FILTER = '$CONSOLE_AGENT_FILTER'" \
  '$exitCode = 1' \
  'try {' \
  "    & '$GUEST_ROOT\\windows-runtime-qualify.ps1' -TargetDir '$GUEST_ROOT\\target' -Mode '$MODE' *> '$LOG'" \
  '    $exitCode = 0' \
  '} catch {' \
  "    \$_ | Out-String | Add-Content -LiteralPath '$LOG'" \
  '    $exitCode = 1' \
  '} finally {' \
  "    [IO.File]::WriteAllText('$RESULT_TMP', [string]\$exitCode)" \
  "    Move-Item -LiteralPath '$RESULT_TMP' -Destination '$RESULT' -Force" \
  '}' \
  'exit $exitCode' | court push "$COURT" - "$JOB"
printf 'ready' | court push "$COURT" - "$READY"

# Each job publishes a unique result path atomically, so no prior run can be
# mistaken for current-source evidence.
deadline="$((SECONDS + 1200))"
while :; do
  : >"$runner_tmp/exit"
  court pull "$COURT" "$RESULT" "$runner_tmp/exit" 2>/dev/null || true
  [ -s "$runner_tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "interactive Windows test job exceeded its 20-minute deadline" >&2
    exit 1
  fi
  sleep 1
done
court pull "$COURT" "$LOG" - || true
runner_rc="$(tr -d '\r\n' <"$runner_tmp/exit")"
case "$runner_rc" in
  ''|*[!0-9]*)
    echo "invalid Windows test result: $runner_rc" >&2
    exit 1
    ;;
esac
exit "$runner_rc"
