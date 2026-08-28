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
COURT_CLI="${MINICON_UTM_COURT_CLI:-$SCRIPT_DIR/utm-court.sh}"
VM="${MINICON_MACOS_UTM_VM:-minicon-osx-arm-64}"
BRIDGE="${MINICON_MACOS_UTM_BRIDGE:-$REPO_ROOT/target-six/macos-utm-bridge}"
BOOTSTRAP_ISO="${MINICON_MACOS_UTM_BOOTSTRAP_ISO:-$REPO_ROOT/target-six/macos-utm-bootstrap.iso}"

[ -x "$UTMCTL" ] || { echo "utmctl not found: $UTMCTL" >&2; exit 2; }
[ -x "$COURT_CLI" ] || { echo "UTM court CLI not found: $COURT_CLI" >&2; exit 2; }
court() {
  UTM_COURT_VM="$VM" UTMCTL="$UTMCTL" UTM_COURT_MACOS_BRIDGE="$BRIDGE" \
    "$COURT_CLI" "$@"
}

if [ "$MODE" = prepare ]; then
  mkdir -p "$BRIDGE/bootstrap" "$BRIDGE/boot-requests" "$BRIDGE/boot-acks" \
    "$BRIDGE/jobs" "$BRIDGE/payloads" "$BRIDGE/results"
  install -m 700 "$SCRIPT_DIR/macos-utm-agent.sh" \
    "$BRIDGE/bootstrap/macos-utm-agent.sh"
  install -m 700 "$SCRIPT_DIR/macos-utm-agent.sh" \
    "$BRIDGE/bootstrap/macos-utm-agent-v2.sh"
  install -m 700 "$SCRIPT_DIR/setup-macos-utm-runner.sh" \
    "$BRIDGE/bootstrap/setup-macos-utm-runner.sh"
  install -m 700 "$SCRIPT_DIR/setup-macos-utm-runner.sh" \
    "$BRIDGE/bootstrap/setup-macos-utm-runner-v2.sh"
  install -m 700 "$SCRIPT_DIR/bootstrap-macos-utm.command" \
    "$BRIDGE/bootstrap/bootstrap-macos-utm.command"
  iso_root="$(mktemp -d)"
  trap 'rm -rf "$iso_root"' EXIT
  install -m 755 "$SCRIPT_DIR/bootstrap-macos-utm.command" \
    "$iso_root/Install MiniCon UTM Agent.command"
  rm -f "$BOOTSTRAP_ISO"
  hdiutil makehybrid -quiet -iso -joliet \
    -default-volume-name MINICON_UTM_BOOTSTRAP \
    -o "$BOOTSTRAP_ISO" "$iso_root"
  printf 'bridge=%s\nbootstrap_iso=%s\n' "$BRIDGE" "$BOOTSTRAP_ISO"
  exit 0
fi

if [ "$MODE" = stop ]; then
  court release osx-aarch64 >/dev/null
  exit $?
fi

mkdir -p "$BRIDGE/boot-requests" "$BRIDGE/boot-acks" \
  "$BRIDGE/jobs" "$BRIDGE/payloads" "$BRIDGE/results"
# Apple Virtualization does not implement UTM disposable start. This clean
# release court uses a cold baseline lease; sealed-clone isolation is the image
# service's owning future boundary.
court lease osx-aarch64 >/dev/null
court wait-ready osx-aarch64 180 >/dev/null

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
  mkdir -p "$payload_tmp/source"
  cp "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/alignment-contract.json" \
    "$REPO_ROOT/evidence-registry.json" "$payload_tmp/source/"
  cp -R "$REPO_ROOT/prd" "$REPO_ROOT/tests" "$payload_tmp/source/"
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
