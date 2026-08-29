#!/bin/bash
# G2 POSIX control black-box against packed minicon.com.
# Isolation: HOME ($HOME/.config/minicon.json) + unique --control.
# Ready: poll list-tabs. PTY proof: echo TOKEN\\r + wait-text + capture/snapshot.
set -euo pipefail
COM="${1:?path to minicon.com}"
test -x "$COM" || chmod +x "$COM"
COM=$(cd "$(dirname "$COM")" && pwd)/$(basename "$COM")
ROOT=$(mktemp -d /tmp/mg2.XXXXXX)
HOME_DIR="$ROOT/home"
WORK="$ROOT/work"
SOCK="$ROOT/c.sock"
ENDPOINT="unix:$SOCK"
HOST_LOG="$ROOT/host.log"
TOKEN_PART="G2TOK$$"
RESULT_TOKEN="G2RESULT${TOKEN_PART}"
cfg_before=""
cfg_after=""
host_pid=""
finished=0
cleanup() {
  if [[ "$finished" -eq 1 ]]; then
    rm -rf "$ROOT"
    return
  fi
  if [[ -n "$host_pid" ]]; then
    "$COM" cli --control "$ENDPOINT" close-window >/dev/null 2>&1 || true
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
  fi
  rm -rf "$ROOT"
}
trap cleanup EXIT

extracts_for() {
  local pid="$1" p
  shopt -s nullglob
  for p in /tmp/minicon.com."${pid}".* /private/tmp/minicon.com."${pid}".*; do
    [[ -e "$p" ]] && printf '%s\n' "$p"
  done
}

cli() {
  "$COM" cli --control "$ENDPOINT" "$@"
}

process_is_live() {
  local state
  state=$(ps -p "$1" -o stat= 2>/dev/null | tr -d ' ' || true)
  [[ -n "$state" && "$state" != Z* ]]
}

mkdir -p "$HOME_DIR/.config" "$WORK"
export HOME="$HOME_DIR"
unset XDG_CONFIG_HOME || true
CFG="$HOME_DIR/.config/minicon.json"
if [[ -e "$CFG" ]]; then
  cfg_before=$(shasum -a 256 "$CFG" | awk '{print $1}')
else
  cfg_before="ABSENT"
fi

cd "$WORK"
"$COM" --no-activate --control "$ENDPOINT" >"$HOST_LOG" 2>&1 &
host_pid=$!
echo "loader_pid=$host_pid"

ready=0
for _ in $(seq 1 150); do
  if cli list-tabs >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! process_is_live "$host_pid"; then
    sed -n '1,120p' "$HOST_LOG" >&2 || true
    echo "FAIL host died before list-tabs" >&2
    exit 1
  fi
  sleep 0.2
done
if [[ "$ready" -ne 1 ]]; then
  sed -n '1,120p' "$HOST_LOG" >&2 || true
  echo "FAIL list-tabs never ready" >&2
  exit 1
fi

tabs=$(cli list-tabs)
echo "list-tabs=$tabs"
tab=$(printf '%s' "$tabs" | python3 -c '
import json,sys
t=json.load(sys.stdin)
tabs=t.get("tabs")
if not isinstance(tabs, list) or not tabs:
    raise SystemExit("list-tabs missing tabs[]")
active=[x for x in tabs if x.get("active") is True]
if len(active)!=1 or not active[0].get("id"):
    raise SystemExit("list-tabs needs exactly one active tab with id")
print(active[0]["id"])
')

# The echoed command line contains TOKEN_PART but never RESULT_TOKEN. Seeing
# RESULT_TOKEN therefore proves that the child shell executed the command; a
# terminal's ordinary input echo cannot satisfy this court.
cli send-text --target "$tab" "printf 'G2RESULT%s\\n' '${TOKEN_PART}'"$'\r'
cli wait-text --target "$tab" --timeout-ms 15000 "$RESULT_TOKEN"

snap=$(cli ui-snapshot)
printf '%s' "$snap" | python3 -c '
import json,sys
s=json.loads(sys.stdin.read())
active=s.get("active")
if active != sys.argv[1]:
    raise SystemExit(f"ui-snapshot active {active!r} != {sys.argv[1]!r}")
print("ui-snapshot-active="+str(active))
' "$tab"

pane=$(cli capture-pane --max-bytes 8000)
if [[ "$pane" != *"$RESULT_TOKEN"* ]]; then
  echo "FAIL capture-pane missing result token: $pane" >&2
  exit 1
fi
echo "capture-pane contains $RESULT_TOKEN"

cli close-window
for _ in $(seq 1 50); do
  if ! process_is_live "$host_pid"; then
    break
  fi
  sleep 0.1
done
if process_is_live "$host_pid"; then
  echo "FAIL loader still alive after close-window" >&2
  exit 1
fi
set +e
wait "$host_pid"
rc=$?
set -e
if [[ "$rc" -ne 0 ]]; then
  echo "FAIL loader rc=$rc want 0" >&2
  exit 1
fi
dead_pid="$host_pid"
host_pid=""

left=$(extracts_for "$dead_pid")
if [[ -n "$left" ]]; then
  echo "FAIL leftover extract for pid $dead_pid:"$'\n'"$left" >&2
  exit 1
fi
echo "extract_dirs_for_$dead_pid=0"

if [[ -e "$CFG" ]]; then
  cfg_after=$(shasum -a 256 "$CFG" | awk '{print $1}')
else
  cfg_after="ABSENT"
fi
echo "config_baseline=$cfg_before"
echo "config_after=$cfg_after"
if [[ "$cfg_before" != "$cfg_after" ]]; then
  echo "FAIL config before!=after (product write or contamination)" >&2
  exit 1
fi

finished=1
echo "PASS g2-control HOME=$HOME_DIR endpoint=$ENDPOINT token=$RESULT_TOKEN"
