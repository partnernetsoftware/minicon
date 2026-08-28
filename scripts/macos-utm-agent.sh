#!/bin/bash
# Login-session worker for MiniCon's clean macOS UTM test target.
# Install this file in the guest and launch it with a user LaunchAgent.

set -u

AUTO_MOUNT="/Volumes/My Shared Files"
if [ -n "${MINICON_MACOS_UTM_MOUNT:-}" ]; then
  MOUNT_POINT="$MINICON_MACOS_UTM_MOUNT"
elif [ -d "$AUTO_MOUNT" ]; then
  if [ -d "$AUTO_MOUNT/macos-utm-bridge/bootstrap" ]; then
    MOUNT_POINT="$AUTO_MOUNT/macos-utm-bridge"
  else
    MOUNT_POINT="$AUTO_MOUNT"
  fi
else
  MOUNT_POINT="$HOME/Library/Caches/minicon-utm-share"
fi
WORK_ROOT="${MINICON_MACOS_UTM_WORK:-$HOME/Library/Caches/minicon-utm-work}"
LOCK_DIR="$WORK_ROOT/agent.lock"

mkdir -p "$WORK_ROOT"
if [ "${MINICON_MACOS_UTM_SKIP_MOUNT:-0}" != 1 ] &&
   [ "$MOUNT_POINT" != "$AUTO_MOUNT" ] &&
   [ "$MOUNT_POINT" != "$AUTO_MOUNT/macos-utm-bridge" ]; then
  mkdir -p "$MOUNT_POINT"
  if ! mount | grep -F " on $MOUNT_POINT " >/dev/null 2>&1; then
    mount_virtiofs share "$MOUNT_POINT" || exit 1
  fi
fi
[ -d "$MOUNT_POINT/bootstrap" ] || exit 1
mkdir -p "$MOUNT_POINT/boot-requests" "$MOUNT_POINT/boot-acks" \
  "$MOUNT_POINT/jobs" "$MOUNT_POINT/results" \
  "$MOUNT_POINT/operations" "$MOUNT_POINT/operation-results"

if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  owner_pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  case "$owner_pid" in
    *[!0-9]*|'') owner_pid= ;;
  esac
  if [ -n "$owner_pid" ] && kill -0 "$owner_pid" 2>/dev/null; then
    exit 0
  fi
  rm -rf "$LOCK_DIR"
  mkdir "$LOCK_DIR" 2>/dev/null || exit 0
fi
printf '%s\n' "$$" >"$LOCK_DIR/pid"
trap 'rm -rf "$LOCK_DIR"' EXIT

while :; do
  for request in "$MOUNT_POINT"/boot-requests/*.ready; do
    [ -f "$request" ] || continue
    token="${request##*/}"
    token="${token%.ready}"
    case "$token" in *[!A-Za-z0-9_-]*|'') continue ;; esac
    printf '2\n' >"$MOUNT_POINT/boot-acks/$token"
  done

  for ready in "$MOUNT_POINT"/operations/*.ready; do
    [ -f "$ready" ] || continue
    operation_id="${ready##*/}"
    operation_id="${operation_id%.ready}"
    case "$operation_id" in *[!A-Za-z0-9_-]*|'') continue ;; esac
    running="$MOUNT_POINT/operations/$operation_id.running"
    mv "$ready" "$running" 2>/dev/null || continue

    operation="$MOUNT_POINT/operations/$operation_id"
    result_dir="$MOUNT_POINT/operation-results/$operation_id"
    result_tmp="$MOUNT_POINT/operation-results/$operation_id.tmp"
    guest_operation="$WORK_ROOT/operations/$operation_id"
    rc=1

    rm -rf "$guest_operation" "$result_tmp"
    mkdir -p "$guest_operation" "$result_tmp"
    if cp -R "$operation/." "$guest_operation/" &&
       (cd "$guest_operation" && shasum -a 256 -c MANIFEST.sha256) \
         >"$result_tmp/verify.log" 2>&1; then
      kind="$(cat "$guest_operation/kind")"
      case "$kind" in
        exec)
          chmod 700 "$guest_operation/command.sh"
          "$guest_operation/command.sh" >"$result_tmp/stdout" \
            2>"$result_tmp/stderr"
          rc=$?
          ;;
        push)
          guest_path="$(cat "$guest_operation/guest-path")"
          case "$guest_path" in *$'\n'*|'') rc=2 ;; *)
            mkdir -p "$(dirname "$guest_path")" &&
              cp "$guest_operation/payload" "$guest_path"
            rc=$?
            ;;
          esac
          ;;
        pull)
          guest_path="$(cat "$guest_operation/guest-path")"
          case "$guest_path" in *$'\n'*|'') rc=2 ;; *)
            cp "$guest_path" "$result_tmp/payload"
            rc=$?
            ;;
          esac
          ;;
        *)
          printf 'unsupported operation kind: %s\n' "$kind" \
            >"$result_tmp/stderr"
          rc=2
          ;;
      esac
    fi
    printf '%s' "$rc" >"$result_tmp/exit"
    rm -rf "$result_dir"
    mv "$result_tmp" "$result_dir"
    rm -f "$running"
  done

  for ready in "$MOUNT_POINT"/jobs/*.ready; do
    [ -f "$ready" ] || continue
    job_id="${ready##*/}"
    job_id="${job_id%.ready}"
    case "$job_id" in
      *[!A-Za-z0-9_-]*|'') continue ;;
    esac
    running="$MOUNT_POINT/jobs/$job_id.running"
    mv "$ready" "$running" 2>/dev/null || continue

    payload="$MOUNT_POINT/payloads/$job_id"
    guest_job="$WORK_ROOT/jobs/$job_id"
    result_tmp="$MOUNT_POINT/results/$job_id.log.tmp"
    result_log="$MOUNT_POINT/results/$job_id.log"
    exit_tmp="$MOUNT_POINT/results/$job_id.exit.tmp"
    exit_file="$MOUNT_POINT/results/$job_id.exit"
    rc=1

    rm -rf "$guest_job"
    mkdir -p "$guest_job"
    if cp -R "$payload/." "$guest_job/" &&
       chmod -R u+rwX "$guest_job" &&
       (cd "$guest_job" && shasum -a 256 -c MANIFEST.sha256) >"$result_tmp" 2>&1; then
      chmod +x "$guest_job/macos-runtime-qualify.sh" \
        "$guest_job/target/debug/minicon" \
        "$guest_job/target/debug/deps/"* \
        "$guest_job/target/release-fast/minicon" \
        "$guest_job/target/release-fast/deps/"* 2>/dev/null || true
      mode="$(sed -n 's/^mode=//p' "$guest_job/job.env")"
      case "$mode" in
        status|test|throughput)
          "$guest_job/macos-runtime-qualify.sh" "$guest_job/target" "$mode" \
            >>"$result_tmp" 2>&1
          rc=$?
          ;;
        *)
          echo "invalid job mode: $mode" >>"$result_tmp"
          ;;
      esac
    fi
    printf '%s' "$rc" >"$exit_tmp"
    mv -f "$result_tmp" "$result_log"
    mv -f "$exit_tmp" "$exit_file"
    rm -f "$running"
  done
  sleep 1
done
