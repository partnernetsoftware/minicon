#!/bin/bash
# Local accelerated lane: one pack on this Mac, then execute on host/Lima/UTM.
# Guests never compile. Lima instances we start, we stop.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
COM="$HERE/dist/minicon.com"
export COSMOCC_DIR="${COSMOCC_DIR:-$HOME/cosmocc}"
export PATH="$COSMOCC_DIR/bin:$PATH"
cd "$ROOT"
PASS=0
FAIL=0
BLOCK=0
STARTED_LIMA=()

record() {
  python3 -c 'import json,sys; print(json.dumps({"cell":sys.argv[1],"status":sys.argv[2],"detail":sys.argv[3],"runner":sys.argv[4]}))' "$1" "$2" "$3" "$4" >>"$RESULTS"
  printf '%-16s %-8s %s\n' "$1" "$2" "$3"
  case "$2" in
    PASS) PASS=$((PASS + 1)) ;;
    FAIL) FAIL=$((FAIL + 1)) ;;
    BLOCKED) BLOCK=$((BLOCK + 1)) ;;
  esac
}

expect() {
  cell="$1"; out="$2"; runner="$3"
  printf '%s\n' "$out" >"$LOGDIR/$cell.log"
  if printf '%s' "$out" | grep -q 'pty backend' && printf '%s' "$out" | grep -q '^minicon '; then
    record "$cell" PASS "$(printf '%s\n' "$out" | sed -n '1p')" "$runner"
  else
    record "$cell" FAIL "$(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)" "$runner"
  fi
}

lima_cell() {
  cell="$1"
  inst="$2"
  command -v limactl >/dev/null || { record "$cell" BLOCKED "limactl missing" "none"; return; }
  running=0
  if limactl list --json "$inst" 2>/dev/null | python3 -c 'import json,sys; raise SystemExit(0 if json.load(sys.stdin).get("status")=="Running" else 1)'; then
    running=1
  fi
  if [[ "$running" -eq 0 ]]; then
    if ! limactl start "$inst" >/dev/null; then
      record "$cell" BLOCKED "limactl start $inst failed" "lima:$inst"
      return
    fi
    STARTED_LIMA+=("$inst")
  fi
  limactl copy "$COM" "$inst:/tmp/minicon.com" >/dev/null
  out="$(limactl shell "$inst" -- sh -c 'chmod +x /tmp/minicon.com && /tmp/minicon.com --status' 2>&1)" || true
  expect "$cell" "$out" "lima:$inst"
}

stop_started_lima() {
  local inst
  for inst in "${STARTED_LIMA[@]+"${STARTED_LIMA[@]}"}"; do
    limactl stop "$inst" >/dev/null 2>&1 || true
  done
}
trap stop_started_lima EXIT

if [[ "${MINICON_COM_SKIP_REBUILD:-}" != 1 ]]; then
  "$HERE/rebuild-payloads.sh"
fi
if [[ ! -x "$COSMOCC_DIR/bin/cosmocc" ]]; then
  "$HERE/install-cosmocc.sh"
fi
"$HERE/pack.sh"
test -x "$COM"
python3 "$HERE/write-build-receipt.py"
LOGDIR="$HERE/dist/cell-logs"
RESULTS="$HERE/dist/cell-results.jsonl"
mkdir -p "$LOGDIR"
: >"$RESULTS"

out="$("$COM" --version 2>&1 | sed -n '1p')"
echo "host_version=$out"
out="$("$COM" --status 2>&1)" && expect osx-aarch64 "$out" "host:$(uname -s)-$(uname -m)" || record osx-aarch64 FAIL "$out" "host"
if arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
  out="$(arch -x86_64 "$COM" --status 2>&1)" && expect osx-x86_64 "$out" "host:rosetta-x86_64" || record osx-x86_64 FAIL "$out" "host:rosetta"
else
  record osx-x86_64 BLOCKED "Rosetta 2 missing" "host"
fi

lima_cell lnx-aarch64 minicon-lnx-aarch64
lima_cell lnx-x86_64 minicon-lnx-x86_64

if [[ -x /Applications/UTM.app/Contents/MacOS/utmctl && -x "$HERE/utm-win-ape-status.sh" ]]; then
  for cell in win-aarch64 win-x86_64; do
    if "$HERE/utm-win-ape-status.sh" "$COM" "$cell" >"$LOGDIR/$cell.log" 2>&1; then
      record "$cell" PASS "UTM job agent" "utm:$cell"
    else
      record "$cell" FAIL "UTM job agent" "utm:$cell"
    fi
  done
else
  record win-aarch64 BLOCKED "utmctl missing" "none"
  record win-x86_64 BLOCKED "utmctl missing" "none"
fi

python3 - "$HERE/dist/build-receipt.json" "$HERE/dist/local-receipt.json" "$LOGDIR" "$RESULTS" "$PASS" "$FAIL" "$BLOCK" <<'PY'
import hashlib, json, sys
from pathlib import Path

build = json.loads(Path(sys.argv[1]).read_text())
out = Path(sys.argv[2])
logdir = Path(sys.argv[3])
rows = []
for line in Path(sys.argv[4]).read_text().splitlines():
    if not line.strip():
        continue
    row = json.loads(line)
    log = logdir / f"{row['cell']}.log"
    if log.is_file():
        raw = log.read_bytes()
        row["log_bytes"] = len(raw)
        row["log_sha256"] = hashlib.sha256(raw).hexdigest()
    rows.append(row)
receipt = {
    "schema": 2,
    "lane": "local-accelerated",
    "source_sha": build["source_sha"],
    "minicon_com_sha256": build["minicon_com_sha256"],
    "product_version": build.get("product_version"),
    "pass": int(sys.argv[5]),
    "fail": int(sys.argv[6]),
    "blocked": int(sys.argv[7]),
    "cells": rows,
}
out.write_text(json.dumps(receipt, indent=2) + "\n")
print(json.dumps({"pass": receipt["pass"], "fail": receipt["fail"], "blocked": receipt["blocked"]}, indent=2))
PY

echo "pass=$PASS fail=$FAIL blocked=$BLOCK"
[[ "$FAIL" -eq 0 ]]
[[ "$PASS" -eq 6 ]]
