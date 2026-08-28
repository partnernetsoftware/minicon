#!/bin/bash
# Container-like lifecycle facade for Lima runtime courts.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="${LIMA_COURT_REGISTRY:-$SCRIPT_DIR/lima-courts.json}"
STATE_DIR="${LIMA_COURT_STATE_DIR:-$SCRIPT_DIR/../target-six/lima-court-service}"
LIMACTL="${LIMACTL:-$(command -v limactl || true)}"
die() { printf 'lima-court: %s\n' "$*" >&2; exit 2; }
blocked() { printf 'lima-court: BLOCKED: %s\n' "$*" >&2; exit 3; }
[ -x "$LIMACTL" ] || die "limactl missing"

resolve() {
  COURT_JSON="$(python3 - "$REGISTRY" "$1" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1], encoding="utf-8"))["courts"]:
    if sys.argv[2] in (c["id"], c["cell"]): print(json.dumps(c)); break
else: raise SystemExit(3)
PY
)" || blocked "unknown court '$1'"
  COURT_ID="$(printf '%s' "$COURT_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
  INSTANCE="$(printf '%s' "$COURT_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["instance"])')"
}
running() {
  "$LIMACTL" list --json "$1" 2>/dev/null |
    python3 -c 'import json,sys; raise SystemExit(0 if json.load(sys.stdin).get("status") == "Running" else 1)'
}
lock() {
  mkdir -p "$STATE_DIR/receipts"
  mkdir "$STATE_DIR/lock" 2>/dev/null || blocked "another lifecycle operation owns the lock"
  trap 'rmdir "$STATE_DIR/lock" 2>/dev/null || true' EXIT
}
finalize() {
  outcome="$1"; [ -f "$STATE_DIR/active.json" ] || return 0
  python3 - "$STATE_DIR/active.json" "$STATE_DIR/receipts" "$outcome" <<'PY'
import datetime,json,os,sys
d=json.load(open(sys.argv[1])); d.update(outcome=sys.argv[3], final_state="stopped", released_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
p=os.path.join(sys.argv[2],d["lease_id"]+".json"); t=p+".tmp"
with open(t,"w") as f: json.dump(d,f,indent=2,sort_keys=True); f.write("\n")
os.replace(t,p)
PY
  rm -f "$STATE_DIR/active.json"
}
release_instance() {
  running "$INSTANCE" || return 0
  "$LIMACTL" stop "$INSTANCE" >/dev/null
  if running "$INSTANCE"; then
    blocked "$COURT_ID did not stop"
  fi
  return 0
}

command="${1:-help}"; [ "$#" -gt 0 ] && shift || true
case "$command" in
  list) python3 -m json.tool "$REGISTRY" ;;
  status)
    resolve "${1:?court required}"
    state=stopped; running "$INSTANCE" && state=started
    printf '{"court":"%s","instance":"%s","state":"%s"}\n' "$COURT_ID" "$INSTANCE" "$state"
    ;;
  image)
    resolve "${1:?court required}"
    python3 - "$COURT_JSON" <<'PY'
import hashlib,json,sys
d=json.loads(sys.argv[1]); raw=json.dumps(d,separators=(",",":"),sort_keys=True).encode(); d["contract_sha256"]=hashlib.sha256(raw).hexdigest(); print(json.dumps(d,separators=(",",":"),sort_keys=True))
PY
    ;;
  lease)
    [ "$#" -eq 1 ] || die "lease requires COURT"; lock; resolve "$1"
    if [ -f "$STATE_DIR/active.json" ]; then
      active_instance="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["instance"])' "$STATE_DIR/active.json")"
      if [ "$active_instance" = "$INSTANCE" ] && running "$INSTANCE"; then cat "$STATE_DIR/active.json"; exit 0; fi
      "$LIMACTL" stop "$active_instance" >/dev/null 2>&1 || true; finalize recovered
    fi
    "$LIMACTL" start "$INSTANCE" >/dev/null
    running "$INSTANCE" || blocked "$COURT_ID failed to start"
    python3 - "$STATE_DIR/active.json.tmp" "$COURT_JSON" <<'PY'
import datetime,json,os,sys
c=json.loads(sys.argv[2]); d={"schema":1,"lease_id":datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")+"-"+str(os.getpid()),"court":c["id"],"cell":c["cell"],"instance":c["instance"],"image":c["image"],"memory_mib":c["memory_mib"],"started_at":datetime.datetime.now(datetime.timezone.utc).isoformat()}
with open(sys.argv[1],"w") as f: json.dump(d,f,indent=2,sort_keys=True); f.write("\n")
PY
    mv "$STATE_DIR/active.json.tmp" "$STATE_DIR/active.json"; cat "$STATE_DIR/active.json"
    ;;
  release)
    [ "$#" -eq 1 ] || die "release requires COURT"; lock; resolve "$1"; release_instance; finalize released
    ;;
  reap)
    lock; [ -f "$STATE_DIR/active.json" ] || exit 0
    resolve "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["court"])' "$STATE_DIR/active.json")"
    release_instance; finalize reaped
    ;;
  exec)
    [ "$#" -ge 3 ] && [ "$2" = -- ] || die "exec requires COURT -- COMMAND"
    resolve "$1"; running "$INSTANCE" || blocked "$COURT_ID is not leased"; shift 2
    "$LIMACTL" shell "$INSTANCE" -- "$@"
    ;;
  *) echo "usage: scripts/lima-court.sh list|status|image|lease|release|reap|exec" >&2; exit 2 ;;
esac
