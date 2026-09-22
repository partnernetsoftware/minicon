#!/bin/bash
# Guest-side idle RSS/CPU for one tinygui pixel-window process.
set -euo pipefail
BIN="${1:?tinygui path}"
SETTLE_S="${TINYGUI_SETTLE_S:-5}"
[ -x "$BIN" ] || { echo "missing $BIN" >&2; exit 2; }

"$BIN" >/tmp/tinygui.guest.log 2>&1 &
pid=$!
sleep "$SETTLE_S"
if ! kill -0 "$pid" 2>/dev/null; then
  echo "TINYGUI_RECEIPT {\"status\":\"exited\"}"
  cat /tmp/tinygui.guest.log >&2 || true
  exit 1
fi
rss_kib=$(ps -p "$pid" -o rss= | tr -d ' ')
pcpu=$(ps -p "$pid" -o pcpu= | tr -d ' ')
kill -TERM "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
printf 'TINYGUI_RECEIPT {"status":"ok","rss_kib":%s,"pcpu":%s}\n' "$rss_kib" "$pcpu"
