#!/bin/bash
# Uniform, product-neutral lifecycle and guest-operation facade for UTM courts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="${UTM_COURT_REGISTRY:-$SCRIPT_DIR/utm-courts.json}"
UTMCTL="${UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
BRIDGE="${UTM_COURT_MACOS_BRIDGE:-$SCRIPT_DIR/../target-six/macos-utm-bridge}"
STATE_DIR="${UTM_COURT_STATE_DIR:-$SCRIPT_DIR/../target-six/utm-court-service}"

usage() {
  cat <<'EOF'
usage: scripts/utm-court.sh COMMAND [ARGS]

Commands:
  list [--json]                       list declared OS/ISA courts
  describe COURT                      print one court as JSON
  check                               validate registry and VM registration
  status COURT                        print normalized court state as JSON
  image COURT                         inspect the cold image contract
  resources                           report declared memory and live court states
  lease COURT [--disposable]          reclaim peer VMs, then start one court
  release COURT                       stop the VM and release host memory
  reap                                release an abandoned active lease
  start COURT [--disposable]          start/resume a court
  wait-ready COURT [SECONDS]          wait for its automation adapter
  interactive-ready COURT [SECONDS]   recover and prove the Windows desktop job agent
  exec COURT -- COMMAND [ARGS...]     execute through QEMU Guest Agent
  push COURT HOST_FILE|- GUEST_PATH   upload an exact file (or stdin)
  pull COURT GUEST_PATH HOST_FILE|-   download atomically (or to stdout)
  idle COURT                          apply the court's declared idle policy
  clone COURT INSTANCE_NAME           clone a stopped sealed baseline

Exit 3 means the declared court or operation is BLOCKED, never skipped.
EOF
}

die() { printf 'utm-court: %s\n' "$*" >&2; exit 2; }
blocked() { printf 'utm-court: BLOCKED: %s\n' "$*" >&2; exit 3; }

# utmctl can wait forever when Apple's VM lifecycle request loses its reply.
# Bound the client call itself; the caller still owns state polling/fallback.
command_bounded() {
  timeout_seconds="$1"
  shift
  # `-c` is deliberate: a here-document would occupy this wrapper's stdin
  # and make every `utmctl file push` publish an empty guest file.
  python3 -c '
import subprocess, sys
try:
    completed = subprocess.run(sys.argv[2:], timeout=int(sys.argv[1]))
except subprocess.TimeoutExpired:
    raise SystemExit(124)
raise SystemExit(completed.returncode)
' "$timeout_seconds" "$@"
}

utmctl_bounded() {
  timeout_seconds="$1"
  shift
  command_bounded "$timeout_seconds" "$UTMCTL" "$@"
}

# UTM 4.7 can print an Apple-event/file error while returning zero. File
# transfer callers must not treat that transport bug as successful evidence.
qga_file_transfer() {
  diagnostic="$(mktemp "${TMPDIR:-/tmp}/utm-court-transfer.XXXXXX")"
  if command_bounded "${UTM_COURT_TRANSFER_TIMEOUT:-30}" "$@" 2>"$diagnostic"; then
    transfer_rc=0
  else
    transfer_rc=$?
  fi
  if [ -s "$diagnostic" ]; then
    cat "$diagnostic" >&2
    if grep -Eiq 'error|failed|timed out|cannot find|not found' "$diagnostic"; then
      transfer_rc=1
    fi
  fi
  rm -f "$diagnostic"
  return "$transfer_rc"
}

# A successful QGA process launch is silent: guest stdout is not transported.
# UTM can nevertheless print an Apple-event/RPC failure and return zero. Keep
# command submission under the same no-false-success rule as file transfer,
# and bound the client itself so recovery cannot hang before its nonce poll.
qga_command() {
  diagnostic="$(mktemp "${TMPDIR:-/tmp}/utm-court-command.XXXXXX")"
  if utmctl_bounded "${UTM_COURT_COMMAND_TIMEOUT:-15}" exec "$VM" --cmd "$@" \
      >"$diagnostic" 2>&1; then
    command_rc=0
  else
    command_rc=$?
  fi
  if [ -s "$diagnostic" ]; then
    cat "$diagnostic" >&2
    command_rc=1
  fi
  rm -f "$diagnostic"
  return "$command_rc"
}

[ -r "$REGISTRY" ] || die "registry missing: $REGISTRY"
[ -x "$UTMCTL" ] || die "utmctl missing: $UTMCTL"

court_json() {
  python3 - "$REGISTRY" "$1" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
for court in data.get("courts", []):
    if court.get("id") == sys.argv[2] or court.get("cell") == sys.argv[2]:
        print(json.dumps(court, separators=(",", ":"), sort_keys=True))
        raise SystemExit(0)
raise SystemExit(3)
PY
}

field() {
  python3 -c 'import json,sys; v=json.load(sys.stdin).get(sys.argv[1]); print("" if v is None else v)' "$1"
}

resolve() {
  COURT_JSON="$(court_json "$1")" || blocked "unknown court '$1'"
  COURT_ID="$(printf '%s' "$COURT_JSON" | field id)"
  VM="$(printf '%s' "$COURT_JSON" | field vm)"
  [ -z "${UTM_COURT_VM:-}" ] || VM="$UTM_COURT_VM"
  ADAPTER="$(printf '%s' "$COURT_JSON" | field adapter)"
  COURT_OS="$(printf '%s' "$COURT_JSON" | field os)"
  INTERACTIVE_USER="$(printf '%s' "$COURT_JSON" | field interactive_user)"
  IDLE="$(printf '%s' "$COURT_JSON" | field idle)"
  TEMPLATE_STATE="$(printf '%s' "$COURT_JSON" | field template_state)"
  [ -n "$VM" ] || blocked "$COURT_ID has no configured VM"
}

stop_and_release() {
  state="$($UTMCTL status "$VM" 2>/dev/null || true)"
  case "$state" in
    stopped) return 0 ;;
    started|suspended)
      utmctl_bounded "${UTM_COURT_COMMAND_TIMEOUT:-15}" stop "$VM" --request || true
      ;;
    *) blocked "$COURT_ID cannot release from state '${state:-unavailable}'" ;;
  esac
  for _ in $(seq 1 "${UTM_COURT_STOP_TIMEOUT:-30}"); do
    [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = stopped ] && return 0
    sleep 1
  done
  # A virtual power event is the bounded fallback; it does not kill UTM or delete disks.
  utmctl_bounded "${UTM_COURT_COMMAND_TIMEOUT:-15}" stop "$VM" --force ||
    blocked "$COURT_ID force-stop command timed out or failed"
  [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = stopped ] ||
    blocked "$COURT_ID did not release host memory"
}

reclaim_peer_vms() {
  while IFS=$'\t' read -r peer_id peer_vm peer_automation_state; do
    [ -n "$peer_vm" ] || continue
    [ "$peer_vm" = "$VM" ] && continue
    # Planned/provisioning VMs are not disposable runtime courts. Stopping one
    # can discard an interactive OS installer at an EULA or partitioning step.
    # Only automation-ready peers participate in single-active memory reclaim.
    [ "$peer_automation_state" = ready ] || continue
    peer_state="$($UTMCTL status "$peer_vm" 2>/dev/null || true)"
    case "$peer_state" in
      started|suspended)
        saved_json="$COURT_JSON"; saved_id="$COURT_ID"; saved_vm="$VM"
        COURT_JSON="$(court_json "$peer_id")"; COURT_ID="$peer_id"; VM="$peer_vm"
        stop_and_release
        COURT_JSON="$saved_json"; COURT_ID="$saved_id"; VM="$saved_vm"
        ;;
    esac
  done < <(python3 - "$REGISTRY" <<'PY'
import json, sys
seen = set()
for c in json.load(open(sys.argv[1], encoding="utf-8"))["courts"]:
    vm = c.get("vm") or ""
    if vm and vm not in seen:
        seen.add(vm)
        print(c["id"], vm, c.get("automation_state", "planned"), sep="\t")
PY
)
}

require_memory_headroom() {
  minimum="${UTM_COURT_MIN_FREE_PERCENT:-20}"
  case "$minimum" in *[!0-9]*|'') die "UTM_COURT_MIN_FREE_PERCENT must be an integer" ;; esac
  available="$(memory_pressure -Q 2>/dev/null | awk '/free percentage:/ {gsub(/%/, "", $NF); print $NF}' || true)"
  [ -z "$available" ] && return 0
  [ "$available" -ge "$minimum" ] ||
    blocked "host memory headroom ${available}% is below required ${minimum}%"
}

service_lock() {
  mkdir -p "$STATE_DIR/receipts"
  if ! mkdir "$STATE_DIR/lock" 2>/dev/null; then
    blocked "another court lifecycle operation owns the service lock"
  fi
  trap 'rmdir "$STATE_DIR/lock" 2>/dev/null || true' EXIT
}

write_active_lease() {
  disposable="$1"
  token="$(date -u +%Y%m%dT%H%M%SZ)-$$-$RANDOM"
  python3 - "$STATE_DIR/active.json.tmp" "$COURT_JSON" "$VM" "$token" "$disposable" <<'PY'
import datetime, json, sys
court = json.loads(sys.argv[2])
data = {
    "schema": 1,
    "lease_id": sys.argv[4],
    "court": court["id"],
    "cell": court["cell"],
    "vm": sys.argv[3],
    "memory_mib": court.get("memory_mib"),
    "image": court.get("image"),
    "disposable": sys.argv[5] == "true",
    "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2, sort_keys=True)
    f.write("\n")
PY
  mv -f "$STATE_DIR/active.json.tmp" "$STATE_DIR/active.json"
  cat "$STATE_DIR/active.json"
}

finalize_lease() {
  outcome="$1"
  [ -f "$STATE_DIR/active.json" ] || return 0
  python3 - "$STATE_DIR/active.json" "$STATE_DIR/receipts" "$outcome" <<'PY'
import datetime, json, os, sys
active = json.load(open(sys.argv[1], encoding="utf-8"))
active["released_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
active["final_state"] = "stopped"
active["outcome"] = sys.argv[3]
target = os.path.join(sys.argv[2], active["lease_id"] + ".json")
temporary = target + ".tmp"
with open(temporary, "w", encoding="utf-8") as f:
    json.dump(active, f, indent=2, sort_keys=True)
    f.write("\n")
os.replace(temporary, target)
PY
  rm -f "$STATE_DIR/active.json"
}

bridge_wait_ready() {
  timeout="$1"
  mkdir -p "$BRIDGE/boot-requests" "$BRIDGE/boot-acks"
  token="court-$COURT_ID-$$-$RANDOM"
  printf ready >"$BRIDGE/boot-requests/$token.ready"
  for _ in $(seq 1 "$timeout"); do
    if [ -f "$BRIDGE/boot-acks/$token" ] &&
       [ "$(cat "$BRIDGE/boot-acks/$token")" = 2 ]; then
      rm -f "$BRIDGE/boot-requests/$token.ready" "$BRIDGE/boot-acks/$token"
      return 0
    fi
    sleep 1
  done
  return 1
}

bridge_publish_operation() {
  operation_id="$1"
  operation_dir="$BRIDGE/operations/$operation_id"
  result_dir="$BRIDGE/operation-results/$operation_id"
  (cd "$operation_dir" && find . -type f ! -name MANIFEST.sha256 -print0 |
    LC_ALL=C sort -z | xargs -0 shasum -a 256) >"$operation_dir/MANIFEST.sha256"
  printf ready >"$BRIDGE/operations/$operation_id.ready"
  for _ in $(seq 1 "${UTM_COURT_OPERATION_TIMEOUT:-300}"); do
    [ -f "$result_dir/exit" ] && break
    sleep 1
  done
  [ -f "$result_dir/exit" ] || blocked "$COURT_ID operation timed out"
  OPERATION_RESULT="$result_dir"
}

normalized_status() {
  raw="$($UTMCTL status "$VM" 2>/dev/null || true)"
  case "$raw" in started|stopped|suspended) ;; *) raw=unavailable ;; esac
  python3 - "$COURT_JSON" "$raw" <<'PY'
import json, sys
court = json.loads(sys.argv[1])
print(json.dumps({
    "adapter": court["adapter"], "automation_state": court["automation_state"],
    "cell": court["cell"],
    "court": court["id"], "isa": court["isa"],
    "state": sys.argv[2], "template_state": court["template_state"],
    "translation": court.get("translation"), "vm": court["vm"]
}, separators=(",", ":"), sort_keys=True))
PY
}

command="${1:-help}"
[ "$#" -gt 0 ] && shift || true
case "$command" in
  help|-h|--help) usage ;;
  list)
    if [ "${1:-}" = --json ]; then
      python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["courts"], indent=2, sort_keys=True))' "$REGISTRY"
    else
      python3 - "$REGISTRY" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1], encoding="utf-8"))["courts"]:
    print(f'{c["id"]:<24} {c["cell"]:<12} {c["adapter"]:<18} {c.get("vm") or "BLOCKED"}')
PY
    fi
    ;;
  describe)
    [ "$#" -eq 1 ] || die "describe requires COURT"
    court_json "$1" || blocked "unknown court '$1'"
    ;;
  check)
    python3 -m json.tool "$REGISTRY" >/dev/null
    rc=0
    while IFS=$'\t' read -r id vm; do
      if [ -z "$vm" ]; then
        printf '%s\tBLOCKED\tno configured VM\n' "$id"
        rc=3
      elif "$UTMCTL" status "$vm" >/dev/null 2>&1; then
        printf '%s\tOK\t%s\n' "$id" "$vm"
      else
        printf '%s\tBLOCKED\tVM not registered: %s\n' "$id" "$vm"
        rc=3
      fi
    done < <(python3 - "$REGISTRY" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1], encoding="utf-8"))["courts"]:
    print(c["id"], c.get("vm") or "", sep="\t")
PY
)
    exit "$rc"
    ;;
  status)
    [ "$#" -eq 1 ] || die "status requires COURT"
    resolve "$1"; normalized_status
    ;;
  image)
    [ "$#" -eq 1 ] || die "image requires COURT"
    resolve "$1"
    state="$($UTMCTL status "$VM" 2>/dev/null || true)"
    python3 - "$COURT_JSON" "$state" "$($UTMCTL version 2>/dev/null || true)" <<'PY'
import hashlib, json, sys
court = json.loads(sys.argv[1])
contract = {
    "schema": 1, "image": court.get("image"), "court": court["id"],
    "cell": court["cell"], "vm": court["vm"], "state": sys.argv[2],
    "template_state": court["template_state"], "adapter": court["adapter"],
    "memory_mib": court.get("memory_mib"), "utm_version": sys.argv[3],
}
canonical = json.dumps(contract, separators=(",", ":"), sort_keys=True).encode()
contract["contract_sha256"] = hashlib.sha256(canonical).hexdigest()
print(json.dumps(contract, separators=(",", ":"), sort_keys=True))
PY
    ;;
  resources)
    [ "$#" -eq 0 ] || die "resources takes no arguments"
    python3 - "$REGISTRY" "$UTMCTL" <<'PY'
import json, subprocess, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
seen = set()
for c in data["courts"]:
    vm = c.get("vm")
    if not vm or vm in seen: continue
    seen.add(vm)
    p = subprocess.run([sys.argv[2], "status", vm], text=True, capture_output=True)
    state = p.stdout.strip() if p.returncode == 0 else "unavailable"
    print(f'{c["id"]}\t{state}\t{c.get("memory_mib", 0)} MiB\t{vm}')
PY
    ;;
  lease)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "lease requires COURT [--disposable]"
    service_lock
    resolve "$1"; option="${2:-}"
    case "$option" in ""|--disposable) ;; *) die "unknown lease option: $option" ;; esac
    if [ -f "$STATE_DIR/active.json" ]; then
      active_vm="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["vm"])' "$STATE_DIR/active.json")"
      active_state="$($UTMCTL status "$active_vm" 2>/dev/null || true)"
      if [ "$active_vm" = "$VM" ] && [ "$active_state" = started ]; then
        cat "$STATE_DIR/active.json"
        exit 0
      fi
      case "$active_state" in started|suspended) "$UTMCTL" stop "$active_vm" --request >/dev/null 2>&1 || true ;; esac
      finalize_lease recovered
    fi
    reclaim_peer_vms
    require_memory_headroom
    "$0" start "$COURT_ID" ${option:+"$option"} >/dev/null
    [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = started ] ||
      blocked "$COURT_ID backend accepted start but did not reach started"
    write_active_lease "$([ "$option" = --disposable ] && printf true || printf false)"
    ;;
  release)
    [ "$#" -eq 1 ] || die "release requires COURT"
    service_lock
    resolve "$1"; stop_and_release; finalize_lease released; normalized_status
    ;;
  reap)
    [ "$#" -eq 0 ] || die "reap takes no arguments"
    service_lock
    [ -f "$STATE_DIR/active.json" ] || exit 0
    active_court="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["court"])' "$STATE_DIR/active.json")"
    resolve "$active_court"; stop_and_release; finalize_lease reaped; normalized_status
    ;;
  start)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "start requires COURT [--disposable]"
    resolve "$1"; option="${2:-}"
    case "$option" in ""|--disposable) ;; *) die "unknown start option: $option" ;; esac
    state="$($UTMCTL status "$VM" 2>/dev/null || true)"
    case "$state" in
      started) ;;
      suspended) "$UTMCTL" start --hide "$VM" ;;
      stopped)
        case "$option" in "") "$UTMCTL" start --hide "$VM" ;; --disposable) "$UTMCTL" start --hide --disposable "$VM" ;; esac
        ;;
      *) blocked "$COURT_ID cannot start from state '${state:-unavailable}'" ;;
    esac
    normalized_status
    ;;
  wait-ready)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "wait-ready requires COURT [SECONDS]"
    resolve "$1"; timeout="${2:-120}"
    case "$timeout" in *[!0-9]*|'') die "SECONDS must be an integer" ;; esac
    if [ "$ADAPTER" = qemu-guest-agent ]; then
      deadline=$((SECONDS + timeout))
      while [ "$SECONDS" -lt "$deadline" ]; do
        remaining=$((deadline - SECONDS))
        probe_timeout="${UTM_COURT_COMMAND_TIMEOUT:-15}"
        [ "$probe_timeout" -le "$remaining" ] || probe_timeout="$remaining"
        UTM_COURT_COMMAND_TIMEOUT="$probe_timeout" qga_command /usr/bin/true 2>/dev/null && { normalized_status; exit 0; }
        remaining=$((deadline - SECONDS))
        [ "$remaining" -gt 0 ] || break
        probe_timeout="${UTM_COURT_COMMAND_TIMEOUT:-15}"
        [ "$probe_timeout" -le "$remaining" ] || probe_timeout="$remaining"
        UTM_COURT_COMMAND_TIMEOUT="$probe_timeout" qga_command cmd.exe /d /c exit 0 2>/dev/null && { normalized_status; exit 0; }
        [ "$SECONDS" -ge "$deadline" ] || sleep 1
      done
      blocked "$COURT_ID Guest Agent did not become ready within ${timeout}s"
    fi
    if [ "$ADAPTER" = virtiofs-agent ]; then
      bridge_wait_ready "$timeout" || blocked "$COURT_ID VirtioFS agent did not become ready within ${timeout}s"
      normalized_status
      exit 0
    fi
    blocked "$COURT_ID has unknown adapter '$ADAPTER'"
    ;;
  interactive-ready)
    [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "interactive-ready requires COURT [SECONDS]"
    resolve "$1"; timeout="${2:-180}"
    case "$timeout" in *[!0-9]*|'') die "SECONDS must be an integer" ;; esac
    [ "$COURT_OS" = win ] || blocked "$COURT_ID is not a Windows court"
    [ "$ADAPTER" = qemu-guest-agent ] || blocked "$COURT_ID has no QGA desktop bridge"
    [ -n "$INTERACTIVE_USER" ] || blocked "$COURT_ID has no interactive_user"
    agent_ps1="$SCRIPT_DIR/windows-utm-agent.ps1"
    [ -f "$agent_ps1" ] || blocked "Windows desktop agent source is missing"

    agent_tag="$$-$RANDOM"
    task_name="UtmCourtInteractiveAgent-$agent_tag"
    guest_ps1="C:\minicon-six\windows-utm-agent.court-$agent_tag.ps1"
    qga_file_transfer "$UTMCTL" file push "$VM" "$guest_ps1" <"$agent_ps1"
    task_command="powershell.exe -NoLogo -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File $guest_ps1"
    # A disposable VM resumes an already logged-in snapshot, so the Startup
    # folder does not run again. QGA itself is session 0 and cannot own GUI
    # evidence. Invoke schtasks directly through QGA: nesting this registration
    # inside session-0 PowerShell/cmd proved guest-agent-version dependent on the
    # x86_64 court. The nonce below remains the authoritative desktop proof.
    if ! qga_command schtasks.exe /create /tn "$task_name" /tr "$task_command" \
      /sc ONLOGON /ru "$INTERACTIVE_USER" /it /f; then
      qga_command schtasks.exe /delete /tn "$task_name" /f || true
      blocked "$COURT_ID could not register its interactive task"
    fi
    if ! qga_command schtasks.exe /run /tn "$task_name"; then
      qga_command schtasks.exe /delete /tn "$task_name" /f || true
      blocked "$COURT_ID could not run its interactive task"
    fi
    agent_root='C:\minicon-six\agent-v2'
    nonce="court-$COURT_ID-$$-$RANDOM"
    probe_dir="$(mktemp -d)"
    trap 'rm -rf "$probe_dir"' EXIT
    printf '%s' "$nonce" >"$probe_dir/nonce"
    deadline=$((SECONDS + timeout))
    published=false
    while [ "$SECONDS" -lt "$deadline" ]; do
      remaining=$((deadline - SECONDS))
      transfer_timeout="${UTM_COURT_TRANSFER_TIMEOUT:-30}"
      [ "$transfer_timeout" -le "$remaining" ] || transfer_timeout="$remaining"
      if UTM_COURT_TRANSFER_TIMEOUT="$transfer_timeout" qga_file_transfer "$UTMCTL" file push "$VM" "$agent_root\ping.request" <"$probe_dir/nonce" 2>/dev/null; then
        published=true
        break
      fi
      [ "$SECONDS" -ge "$deadline" ] || sleep 1
    done
    if [ "$published" != true ]; then
      qga_command schtasks.exe /delete /tn "$task_name" /f || true
      blocked "$COURT_ID interactive agent did not publish its protocol root within ${timeout}s"
    fi
    while [ "$SECONDS" -lt "$deadline" ]; do
      : >"$probe_dir/pong"
      remaining=$((deadline - SECONDS))
      transfer_timeout="${UTM_COURT_TRANSFER_TIMEOUT:-30}"
      [ "$transfer_timeout" -le "$remaining" ] || transfer_timeout="$remaining"
      UTM_COURT_TRANSFER_TIMEOUT="$transfer_timeout" qga_file_transfer "$UTMCTL" file pull "$VM" "$agent_root\ping.response" \
        >"$probe_dir/pong" 2>/dev/null || true
      if [ "$(cat "$probe_dir/pong")" = "$nonce" ]; then
          qga_command schtasks.exe /delete /tn "$task_name" /f || true
          python3 - "$COURT_JSON" "$nonce" <<'PY'
import json, sys
court = json.loads(sys.argv[1])
print(json.dumps({
    "adapter": "interactive-job-agent", "cell": court["cell"],
    "court": court["id"], "nonce": sys.argv[2], "status": "ready",
}, separators=(",", ":"), sort_keys=True))
PY
          exit 0
      fi
      [ "$SECONDS" -ge "$deadline" ] || sleep 1
    done
    qga_command schtasks.exe /delete /tn "$task_name" /f || true
    blocked "$COURT_ID interactive job agent did not claim its nonce within ${timeout}s"
    ;;
  exec)
    [ "$#" -ge 3 ] && [ "$2" = -- ] || die "exec requires COURT -- COMMAND [ARGS...]"
    resolve "$1"; shift 2
    if [ "$ADAPTER" = qemu-guest-agent ]; then
      "$UTMCTL" exec "$VM" --cmd "$@"
      exit $?
    fi
    operation_id="exec-$$-$RANDOM"; operation_dir="$BRIDGE/operations/$operation_id"
    mkdir -p "$operation_dir" "$BRIDGE/operation-results"
    printf exec >"$operation_dir/kind"
    { printf '#!/bin/bash\nexec'; for arg in "$@"; do printf ' %q' "$arg"; done; printf '\n'; } \
      >"$operation_dir/command.sh"
    bridge_publish_operation "$operation_id"
    cat "$OPERATION_RESULT/stdout" 2>/dev/null || true
    cat "$OPERATION_RESULT/stderr" >&2 2>/dev/null || true
    exit "$(cat "$OPERATION_RESULT/exit")"
    ;;
  push)
    [ "$#" -eq 3 ] || die "push requires COURT HOST_FILE GUEST_PATH"
    resolve "$1"
    [ "$2" = - ] || [ -f "$2" ] || die "host file missing: $2"
    if [ "$ADAPTER" = qemu-guest-agent ]; then
      if [ "$2" = - ]; then qga_file_transfer "$UTMCTL" file push "$VM" "$3"; else qga_file_transfer "$UTMCTL" file push "$VM" "$3" <"$2"; fi
      exit $?
    fi
    operation_id="push-$$-$RANDOM"; operation_dir="$BRIDGE/operations/$operation_id"
    mkdir -p "$operation_dir" "$BRIDGE/operation-results"
    printf push >"$operation_dir/kind"; printf '%s' "$3" >"$operation_dir/guest-path"
    if [ "$2" = - ]; then cat >"$operation_dir/payload"; else cp "$2" "$operation_dir/payload"; fi
    bridge_publish_operation "$operation_id"; exit "$(cat "$OPERATION_RESULT/exit")"
    ;;
  pull)
    [ "$#" -eq 3 ] || die "pull requires COURT GUEST_PATH HOST_FILE"
    resolve "$1"
    if [ "$ADAPTER" = qemu-guest-agent ]; then
      if [ "$3" = - ]; then qga_file_transfer "$UTMCTL" file pull "$VM" "$2"; exit $?; fi
      tmp="$3.tmp.$$"; trap 'rm -f "$tmp"' EXIT
      qga_file_transfer "$UTMCTL" file pull "$VM" "$2" >"$tmp" || exit $?
      mv -f "$tmp" "$3"; exit 0
    fi
    operation_id="pull-$$-$RANDOM"; operation_dir="$BRIDGE/operations/$operation_id"
    mkdir -p "$operation_dir" "$BRIDGE/operation-results"
    printf pull >"$operation_dir/kind"; printf '%s' "$2" >"$operation_dir/guest-path"
    bridge_publish_operation "$operation_id"
    [ "$(cat "$OPERATION_RESULT/exit")" = 0 ] || exit "$(cat "$OPERATION_RESULT/exit")"
    if [ "$3" = - ]; then cat "$OPERATION_RESULT/payload"; exit 0; fi
    tmp="$3.tmp.$$"; trap 'rm -f "$tmp"' EXIT
    cp "$OPERATION_RESULT/payload" "$tmp"; mv -f "$tmp" "$3"
    ;;
  idle)
    [ "$#" -eq 1 ] || die "idle requires COURT"
    resolve "$1"
    state="$($UTMCTL status "$VM" 2>/dev/null || true)"
    case "$IDLE:$state" in
      suspend:started) "$UTMCTL" suspend "$VM" ;;
      suspend:suspended|suspend:stopped|stop:stopped) ;;
      stop:started|stop:suspended) stop_and_release ;;
      suspend:*|stop:*) blocked "$COURT_ID cannot idle from state '${state:-unavailable}'" ;;
      *) die "invalid idle policy '$IDLE'" ;;
    esac
    normalized_status
    ;;
  clone)
    [ "$#" -eq 2 ] || die "clone requires COURT INSTANCE_NAME"
    resolve "$1"
    case "$TEMPLATE_STATE" in sealed*) ;; *) blocked "$COURT_ID is not a sealed template" ;; esac
    [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = stopped ] || blocked "$COURT_ID baseline must be stopped before clone"
    "$UTMCTL" clone "$VM" --name "$2"
    ;;
  *) usage >&2; exit 2 ;;
esac
