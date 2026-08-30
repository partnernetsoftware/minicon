#!/bin/bash
# Qualify one already-linked Linux release binary in two clean Ubuntu courts:
# missing runtime dependency (actionable failure) and runtime-only X11 startup.

set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: scripts/linux-x11-package-smoke.sh PRODUCT" >&2
  exit 2
fi

PRODUCT="$1"
[ -x "$PRODUCT" ] || {
  echo "missing executable Linux product: $PRODUCT" >&2
  exit 2
}
command -v docker >/dev/null 2>&1 || {
  echo "required package-court tool missing: docker" >&2
  exit 2
}

PRODUCT_DIR="$(cd "$(dirname "$PRODUCT")" && pwd)"
PRODUCT_NAME="$(basename "$PRODUCT")"
IMAGE="ubuntu:24.04"

missing_log="$(mktemp "${TMPDIR:-/tmp}/minicon-xkb-missing.XXXXXX")"
runtime_script="$(mktemp "${TMPDIR:-/tmp}/minicon-xkb-runtime.XXXXXX")"
cleanup() {
  rm -f "$missing_log" "$runtime_script"
}
trap cleanup EXIT HUP INT TERM

set +e
docker run --rm \
  --env DISPLAY=:99 \
  --volume "$PRODUCT_DIR:/court:ro" \
  "$IMAGE" "/court/$PRODUCT_NAME" >"$missing_log" 2>&1
missing_rc=$?
set -e
[ "$missing_rc" -eq 1 ] || {
  echo "missing-runtime court expected exit 1, got $missing_rc" >&2
  cat "$missing_log" >&2
  exit 1
}
grep -F "Linux X11 runtime dependency unavailable: libxkbcommon-x11.so.0" "$missing_log" >/dev/null
grep -F "apt-get install libxkbcommon-x11-0" "$missing_log" >/dev/null
grep -F "The -dev package is not required" "$missing_log" >/dev/null
if grep -E "panicked at|(/Users|/home)/[^/]+/[.]cargo" "$missing_log" >/dev/null; then
  echo "missing-runtime court leaked a panic or build-host path" >&2
  cat "$missing_log" >&2
  exit 1
fi

printf '%s\n' \
  'set -euo pipefail' \
  'trap '\''rc=$?; echo "runtime court failed at line $LINENO: $BASH_COMMAND" >&2; cat /tmp/minicon.err 2>/dev/null >&2 || true; exit "$rc"'\'' ERR' \
  'export DEBIAN_FRONTEND=noninteractive' \
  'apt-get update -qq' \
  'apt-get install -y -qq dbus-x11 fonts-dejavu-core libwayland-client0 libx11-6 libxcursor1 libxi6 libxinerama1 libxkbcommon0 libxkbcommon-x11-0 libxrandr2 xvfb >/dev/null' \
  '! dpkg-query -W libxkbcommon-x11-dev >/dev/null 2>&1' \
  'ldconfig -p >/tmp/ldconfig.txt' \
  'grep -F "libxkbcommon-x11.so.0" /tmp/ldconfig.txt >/dev/null' \
  'test -z "$(find /usr/lib /lib \( -type f -o -type l \) -name "libxkbcommon-x11.so" -print -quit 2>/dev/null)"' \
  'court_dir=$(mktemp -d /tmp/minicon-x11-court.XXXXXX)' \
  'chmod 700 "$court_dir"' \
  'endpoint="unix:$court_dir/control.sock"' \
  'xvfb-run -a -s "-screen 0 1280x900x24" dbus-run-session -- /court/minicon --control "$endpoint" >/tmp/minicon.out 2>/tmp/minicon.err &' \
  'pid=$!' \
  'ready=0' \
  'for _ in $(seq 1 100); do' \
  '  if /court/minicon cli --control "$endpoint" ui-snapshot >/tmp/snapshot.json 2>/dev/null; then ready=1; break; fi' \
  '  kill -0 "$pid" 2>/dev/null || break' \
  '  sleep 0.1' \
  'done' \
  '[ "$ready" -eq 1 ] || { cat /tmp/minicon.err >&2; exit 1; }' \
  'grep -F '"'"'"active": "@1"'"'"' /tmp/snapshot.json >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Ctrl+Shift+I >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-ime commit "printf XKB_RUNTIME_ONLY_OK" >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Enter >/dev/null' \
  '/court/minicon cli --control "$endpoint" ui-snapshot | grep -F '"'"'"composer_text": "printf XKB_RUNTIME_ONLY_OK\\n"'"'"' >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Ctrl+O >/dev/null' \
  '/court/minicon cli --control "$endpoint" wait-text --target @1 --timeout-ms 3000 XKB_RUNTIME_ONLY_OK >/dev/null' \
  '/court/minicon cli --control "$endpoint" close-tab --target @1 >/dev/null' \
  '/court/minicon cli --control "$endpoint" ui-snapshot | grep -F '"'"'"workspace_empty": true'"'"' >/dev/null' \
  '/court/minicon cli --control "$endpoint" close-window >/dev/null' \
  'wait "$pid"' \
  '! grep -E "panicked at|Linux X11 runtime dependency unavailable|(/Users|/home)/[^/]+/[.]cargo" /tmp/minicon.err >/dev/null' >"$runtime_script"

docker run --rm \
  --volume "$PRODUCT_DIR:/court:ro" \
  --volume "$runtime_script:/runtime-court.sh:ro" \
  "$IMAGE" bash /runtime-court.sh

echo "linux X11 runtime-only package court: PASS"
