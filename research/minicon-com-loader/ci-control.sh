#!/bin/bash
# G2 POSIX control black-box against packed minicon.com.
# Isolation: HOME ($HOME/.config/minicon.json) + unique --control.
# Ready: poll list-tabs. PTY proof: echo TOKEN\\r + wait-text + capture/snapshot.
set -euo pipefail
COM="${1:?path to minicon.com}"
test -x "$COM" || chmod +x "$COM"
ROOT=$(mktemp -d /tmp/mg2.XXXXXX)
HOME_DIR="$ROOT/home"
WORK="$ROOT/work"
SOCK="$ROOT/c.sock"
ENDPOINT="unix:$SOCK"
TOKEN="G2TOK$$"
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
"$COM" --no-activate --control "$ENDPOINT" >/dev/null 2>&1 &
host_pid=$!
echo "loader_pid=$host_pid"

ready=0
for _ in $(seq 1 150); do
  if cli list-tabs >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! kill -0 "$host_pid" 2>/dev/null; then
    echo "FAIL host died before list-tabs" >&2
    exit 1
  fi
  sleep 0.2
done
if [[ "$ready" -ne 1 ]]; then
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

cli send-text --target "$tab" "echo ${TOKEN}"$'\r'
cli wait-text --target "$tab" --timeout-ms 15000 "$TOKEN"

snap=$(cli ui-snapshot)
printf '%s' "$snap" | python3 -c '
import json,sys
s=json.loads(sys.stdin.read())
if "active" not in s:
    raise SystemExit("ui-snapshot JSON missing active")
print("ui-snapshot-active="+str(s.get("active")))
'

pane=$(cli capture-pane --max-bytes 8000)
if [[ "$pane" != *"$TOKEN"* ]]; then
  echo "FAIL capture-pane missing token: $pane" >&2
  exit 1
fi
echo "capture-pane contains $TOKEN"

cli close-window
for _ in $(seq 1 50); do
  if ! kill -0 "$host_pid" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
set +e
wait "$host_pid"
rc=$?
set -e
if kill -0 "$host_pid" 2>/dev/null; then
  echo "FAIL loader still alive after close-window" >&2
  exit 1
fi
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
echo "PASS g2-control HOME=$HOME_DIR endpoint=$ENDPOINT token=$TOKEN"
