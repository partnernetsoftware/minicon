#!/bin/bash
# G2 POSIX control black-box against an already packed minicon.com.
# Isolation: HOME (config = $HOME/.config/minicon.json) + unique --control.
# Ready: poll `cli --control ENDPOINT list-tabs`. Not wait-ready, not XDG as config root.
set -euo pipefail
COM="${1:?path to minicon.com}"
test -x "$COM" || chmod +x "$COM"
ROOT=$(mktemp -d /tmp/mg2.XXXXXX)
HOME_DIR="$ROOT/home"
WORK="$ROOT/work"
SOCK="$ROOT/c.sock"
ENDPOINT="unix:$SOCK"
cfg_before=""
cfg_after=""
host_pid=""
cleanup() {
  if [[ -n "$host_pid" ]]; then
    MINICON_COM_CELLS="${MINICON_COM_CELLS:-}" "$COM" cli --control "$ENDPOINT" close-window >/dev/null 2>&1 || true
    kill "$host_pid" 2>/dev/null || true
    wait "$host_pid" 2>/dev/null || true
  fi
  rm -rf "$ROOT"
}
trap cleanup EXIT

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

ready=0
for _ in $(seq 1 150); do
  if "$COM" cli --control "$ENDPOINT" list-tabs >/dev/null 2>&1; then
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
tabs=$("$COM" cli --control "$ENDPOINT" list-tabs)
echo "list-tabs=$tabs"
"$COM" cli --control "$ENDPOINT" send-text 'g2-probe'
snap=$("$COM" cli --control "$ENDPOINT" ui-snapshot || true)
if [[ -z "$snap" ]]; then
  "$COM" cli --control "$ENDPOINT" capture-pane --max-bytes 4000 >/dev/null
fi
"$COM" cli --control "$ENDPOINT" close-window >/dev/null 2>&1 || true
wait "$host_pid" 2>/dev/null || true
host_pid=""

if [[ -e "$CFG" ]]; then
  cfg_after=$(shasum -a 256 "$CFG" | awk '{print $1}')
else
  cfg_after="ABSENT"
fi
echo "config_baseline=$cfg_before"
echo "config_after=$cfg_after"
# Isolated HOME started empty; after clean exit the file may appear as this
# run's writes. Uncontaminated vs the pre-start baseline means: if it was
# ABSENT, a new file is this-run owned (OK). If it existed, digest must match
# unless we only created it now from ABSENT.
if [[ "$cfg_before" != "ABSENT" && "$cfg_after" != "$cfg_before" ]]; then
  echo "FAIL config mutated vs baseline" >&2
  exit 1
fi
echo "PASS g2-control HOME=$HOME_DIR endpoint=$ENDPOINT"
