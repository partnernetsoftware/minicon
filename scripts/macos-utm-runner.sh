#!/bin/bash
# Bridge an exact host-linked macOS artifact set into a clean UTM macOS VM.

set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: scripts/macos-utm-runner.sh osx-aarch64 TARGET_DIR prepare|status|test|throughput|stop" >&2
  exit 2
fi

CELL="$1"
TARGET_DIR="$2"
MODE="$3"
[ "$CELL" = osx-aarch64 ] || {
  echo "unsupported macOS UTM cell: $CELL" >&2
  exit 2
}
case "$MODE" in prepare|status|test|throughput|stop) ;; *) exit 2 ;; esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
UTMCTL="${MINICON_UTMCTL:-/Applications/UTM.app/Contents/MacOS/utmctl}"
VM="${MINICON_MACOS_UTM_VM:-minicon-osx-arm64}"
BRIDGE="${MINICON_MACOS_UTM_BRIDGE:-$REPO_ROOT/target-six/macos-utm-bridge}"

[ -x "$UTMCTL" ] || { echo "utmctl not found: $UTMCTL" >&2; exit 2; }

if [ "$MODE" = prepare ]; then
  mkdir -p "$BRIDGE/bootstrap" "$BRIDGE/jobs" "$BRIDGE/payloads" "$BRIDGE/results"
  install -m 700 "$SCRIPT_DIR/macos-utm-agent.sh" \
    "$BRIDGE/bootstrap/macos-utm-agent.sh"
  install -m 700 "$SCRIPT_DIR/setup-macos-utm-runner.sh" \
    "$BRIDGE/bootstrap/setup-macos-utm-runner.sh"
  printf '%s\n' "$BRIDGE"
  exit 0
fi

if [ "$MODE" = stop ]; then
  status="$($UTMCTL status "$VM" 2>/dev/null || true)"
  if [ "$status" != stopped ]; then
    "$UTMCTL" stop "$VM" >/dev/null 2>&1 || true
    for _ in $(seq 1 90); do
      [ "$($UTMCTL status "$VM" 2>/dev/null || true)" = stopped ] && exit 0
      sleep 1
    done
    echo "UTM VM did not stop within 90 seconds: $VM" >&2
    exit 1
  fi
  exit 0
fi

mkdir -p "$BRIDGE/jobs" "$BRIDGE/payloads" "$BRIDGE/results"
status="$($UTMCTL status "$VM" 2>/dev/null || true)"
case "$status" in
  started) ;;
  stopped)
    if [ "${MINICON_MACOS_UTM_DISPOSABLE:-1}" = 1 ]; then
      "$UTMCTL" start --hide --disposable "$VM" >/dev/null 2>&1
    else
      "$UTMCTL" start --hide "$VM" >/dev/null 2>&1
    fi
    ;;
  suspended) "$UTMCTL" start --hide "$VM" >/dev/null 2>&1 ;;
  *) echo "cannot start UTM VM '$VM' from status '${status:-unknown}'" >&2; exit 1 ;;
esac

boot_token="minicon-$CELL-$$-$RANDOM"
printf '%s' "$boot_token" >"$BRIDGE/boot-request.tmp"
mv -f "$BRIDGE/boot-request.tmp" "$BRIDGE/boot-request"
ready=0
for _ in $(seq 1 180); do
  if [ -f "$BRIDGE/agent-ready" ] &&
     [ "$(cat "$BRIDGE/agent-ready")" = "$boot_token" ]; then
    ready=1
    break
  fi
  sleep 1
done
[ "$ready" -eq 1 ] || { echo "macOS guest login agent did not become ready" >&2; exit 1; }

if [ "$MODE" = throughput ]; then PROFILE=release-fast; else PROFILE=debug; fi
HOST_TARGET="$REPO_ROOT/$TARGET_DIR"
HOST_PROFILE="$HOST_TARGET/$PROFILE"
[ -x "$HOST_PROFILE/minicon" ] || { echo "missing artifact: $HOST_PROFILE/minicon" >&2; exit 2; }
identity="$(printf '%s\n' "$TARGET_DIR" | sed -n 's#.*target-six/builds/\([0-9a-f]\{64\}\)/.*#\1#p')"
[ -n "$identity" ] || { echo "target directory lacks source fingerprint" >&2; exit 2; }
current_identity="$(cd "$REPO_ROOT" && python3 scripts/source-fingerprint.py |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["sha256"])')"
[ "$identity" = "$current_identity" ] || {
  echo "artifact source fingerprint is stale: $identity != $current_identity" >&2
  exit 2
}

job_id="${CELL//-/_}_${MODE}_${identity}_$$_$RANDOM"
payload_tmp="$BRIDGE/payloads/$job_id.tmp"
payload="$BRIDGE/payloads/$job_id"
rm -rf "$payload_tmp" "$payload"
mkdir -p "$payload_tmp/target/$PROFILE/deps"
cp "$HOST_PROFILE/minicon" "$payload_tmp/target/$PROFILE/minicon"
cp "$SCRIPT_DIR/macos-runtime-qualify.sh" "$payload_tmp/macos-runtime-qualify.sh"
printf 'mode=%s\n' "$MODE" >"$payload_tmp/job.env"

if [ "$MODE" = test ]; then
  prefixes="minicon minicon_core minicon_alignment minicon_load_portability minicon_console_agent minicon_control minicon_blackbox"
else
  prefixes="minicon_throughput"
fi
if [ "$MODE" != status ]; then
  for prefix in $prefixes; do
    matches=()
    while IFS= read -r match; do
      list_output="$("$match" --list 2>/dev/null || true)"
      printf '%s\n' "$list_output" | tail -n 1 | grep -E '[0-9]+ tests?, [0-9]+ benchmarks?$' \
        >/dev/null 2>&1 && matches+=("$match")
    done < <(find "$HOST_PROFILE/deps" -maxdepth 1 -type f -perm -111 \
      -name "$prefix-[0-9a-f]*" -print)
    [ "${#matches[@]}" -eq 1 ] || {
      echo "expected one $prefix harness, found ${#matches[@]}" >&2
      exit 2
    }
    cp "${matches[0]}" "$payload_tmp/target/$PROFILE/deps/"
  done
fi
(cd "$payload_tmp" && find . -type f ! -name MANIFEST.sha256 -print0 | sort -z |
  xargs -0 shasum -a 256 >MANIFEST.sha256)
mv "$payload_tmp" "$payload"

rm -f "$BRIDGE/results/$job_id.log" "$BRIDGE/results/$job_id.exit"
printf ready >"$BRIDGE/jobs/$job_id.ready.tmp"
mv "$BRIDGE/jobs/$job_id.ready.tmp" "$BRIDGE/jobs/$job_id.ready"
deadline=$((SECONDS + 1200))
while [ ! -s "$BRIDGE/results/$job_id.exit" ]; do
  [ "$SECONDS" -lt "$deadline" ] || { echo "macOS UTM court exceeded 20 minutes" >&2; exit 1; }
  sleep 1
done
cat "$BRIDGE/results/$job_id.log"
rc="$(tr -d '\r\n' <"$BRIDGE/results/$job_id.exit")"
case "$rc" in ''|*[!0-9]*) exit 1 ;; esac
exit "$rc"
