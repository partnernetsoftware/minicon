#!/bin/bash
# Product-neutral lifecycle contract test; never starts a real VM.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/state"

python3 - "$scratch/registry.json" <<'PY'
import json, sys
data = {"schema": 2, "courts": [
    {"id":"one", "cell":"osx-aarch64", "os":"osx", "isa":"aarch64",
     "vm":"vm-one", "image":"test/one:sealed", "adapter":"qemu-guest-agent",
     "automation_state":"ready", "idle":"stop", "memory_mib":1024,
     "template_state":"sealed-test"},
    {"id":"two", "cell":"win-aarch64", "os":"win", "isa":"aarch64",
     "vm":"vm-two", "image":"test/two:sealed", "adapter":"qemu-guest-agent",
     "automation_state":"ready", "idle":"stop", "memory_mib":1024,
     "template_state":"sealed-test"},
    {"id":"provisioning", "cell":"win-x86_64", "os":"win", "isa":"x86_64",
     "vm":"vm-provisioning", "image":"test/provisioning:planned", "adapter":"qemu-guest-agent",
     "automation_state":"planned", "idle":"stop", "memory_mib":1024,
     "template_state":"planned"},
]}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(data, f)
PY

sed "s|@STATE@|$scratch/state|g" >"$scratch/utmctl" <<'SH'
#!/bin/bash
set -eu
state_dir="@STATE@"
command="$1"; shift
case "$command" in
  version) printf 'selftest-1\n' ;;
  status) test -f "$state_dir/$1" && cat "$state_dir/$1" || printf 'stopped\n' ;;
  start)
    vm="${!#}"
    printf 'started\n' >"$state_dir/$vm"
    ;;
  stop)
    vm="$1"
    if [ "${2:-}" = --request ] && [ -f "$state_dir/hang-request" ]; then
      sleep 10
    fi
    printf 'stopped\n' >"$state_dir/$vm"
    ;;
  exec)
    if [ -f "$state_dir/exec-diagnostic" ]; then
      printf 'Error from event: RPC failed\n' >&2
      exit 0
    fi
    ;;
  file)
    action="$1"; path="$3"
    case "$path" in
      *missing-parent*|*missing-file*)
        printf "failed to open file '%s': not found\n" "$path" >&2
        exit 0
        ;;
    esac
    case "$action" in push) cat >/dev/null ;; pull) printf fixture ;; *) exit 2 ;; esac
    ;;
  clone) ;;
  *) exit 2 ;;
esac
SH
chmod +x "$scratch/utmctl"
printf 'started\n' >"$scratch/state/vm-provisioning"

court() {
  UTMCTL="$scratch/utmctl" \
  UTM_COURT_COMMAND_TIMEOUT="${UTM_COURT_COMMAND_TIMEOUT:-15}" \
  UTM_COURT_REGISTRY="$scratch/registry.json" \
  UTM_COURT_STATE_DIR="$scratch/service" \
  "$SCRIPT_DIR/utm-court.sh" "$@"
}

court lease one --disposable >"$scratch/first.json"
court lease one --disposable >"$scratch/reused.json"
cmp "$scratch/first.json" "$scratch/reused.json"

court lease two --disposable >"$scratch/second.json"
[ "$(cat "$scratch/state/vm-one")" = stopped ]
[ "$(cat "$scratch/state/vm-two")" = started ]
[ "$(cat "$scratch/state/vm-provisioning")" = started ]
court release two >/dev/null
[ "$(cat "$scratch/state/vm-two")" = stopped ]
[ ! -e "$scratch/service/active.json" ]

printf fixture >"$scratch/input"
if court push two "$scratch/input" 'C:\missing-parent\file'; then
  echo "utm-court-selftest: zero-exit failed push was accepted" >&2
  exit 1
fi
if court pull two 'C:\missing-file' "$scratch/output"; then
  echo "utm-court-selftest: zero-exit failed pull was accepted" >&2
  exit 1
fi
[ ! -e "$scratch/output" ]

# A zero process status plus an RPC diagnostic is not QGA readiness.
touch "$scratch/state/exec-diagnostic"
if court wait-ready two 1; then
  echo "utm-court-selftest: zero-exit failed exec was accepted" >&2
  exit 1
fi
rm "$scratch/state/exec-diagnostic"

# A lost Apple-event reply must not hold the lifecycle service forever. The
# bounded request falls through to the existing state poll and force-stop.
court lease two >/dev/null
touch "$scratch/state/hang-request"
started_at=$SECONDS
UTM_COURT_COMMAND_TIMEOUT=1 UTM_COURT_STOP_TIMEOUT=1 court release two >/dev/null
[ "$((SECONDS - started_at))" -lt 5 ]
[ "$(cat "$scratch/state/vm-two")" = stopped ]
rm "$scratch/state/hang-request"

court lease one >/dev/null
court reap >/dev/null
[ "$(cat "$scratch/state/vm-one")" = stopped ]
[ ! -e "$scratch/service/active.json" ]

python3 - "$scratch/service/receipts" <<'PY'
import json, pathlib, sys
receipts = [json.load(open(p, encoding="utf-8")) for p in pathlib.Path(sys.argv[1]).glob("*.json")]
outcomes = sorted(r["outcome"] for r in receipts)
assert outcomes == ["reaped", "recovered", "released", "released"], outcomes
assert all(r["final_state"] == "stopped" for r in receipts)
PY
printf 'utm-court-selftest: PASS\n'
