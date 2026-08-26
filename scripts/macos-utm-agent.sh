#!/bin/bash
# Login-session worker for MiniCon's clean macOS UTM test target.
# Install this file in the guest and launch it with a user LaunchAgent.

set -u

MOUNT_POINT="${MINICON_MACOS_UTM_MOUNT:-$HOME/Library/Caches/minicon-utm-share}"
WORK_ROOT="${MINICON_MACOS_UTM_WORK:-$HOME/Library/Caches/minicon-utm-work}"
LOCK_DIR="$WORK_ROOT/agent.lock"

mkdir -p "$MOUNT_POINT" "$WORK_ROOT"
if ! mount | grep -F " on $MOUNT_POINT " >/dev/null 2>&1; then
  mount_virtiofs share "$MOUNT_POINT" || exit 1
fi
mkdir -p "$MOUNT_POINT/jobs" "$MOUNT_POINT/results"

if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  exit 0
fi
trap 'rmdir "$LOCK_DIR" 2>/dev/null || true' EXIT

while :; do
  if [ -f "$MOUNT_POINT/boot-request" ]; then
    cp "$MOUNT_POINT/boot-request" "$MOUNT_POINT/agent-ready.tmp" &&
      mv -f "$MOUNT_POINT/agent-ready.tmp" "$MOUNT_POINT/agent-ready"
  fi

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
