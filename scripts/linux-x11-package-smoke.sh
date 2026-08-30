#!/bin/bash
# Qualify one already-linked Linux release binary on a slim X11 desktop that
# ships libxkbcommon0 but not libxkbcommon-x11-0.

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
runtime_script="$(mktemp "${TMPDIR:-/tmp}/minicon-xkb-runtime.XXXXXX")"
cleanup() {
  rm -f "$runtime_script"
}
trap cleanup EXIT HUP INT TERM

if readelf -d "$PRODUCT" >/tmp/minicon-readelf.txt 2>&1; then
  if grep -F "libxkbcommon-x11.so" /tmp/minicon-readelf.txt >/dev/null; then
    echo "product ELF must not list libxkbcommon-x11 as a load-time NEEDED dependency" >&2
    cat /tmp/minicon-readelf.txt >&2
    exit 1
  fi
else
  echo "readelf is required to gate load-time libxkbcommon-x11 imports" >&2
  exit 2
fi

printf '%s\n' \
  'set -euo pipefail' \
  'trap '\''rc=$?; echo "runtime court failed at line $LINENO: $BASH_COMMAND" >&2; cat /tmp/minicon.err 2>/dev/null >&2 || true; exit "$rc"'\'' ERR' \
  'export DEBIAN_FRONTEND=noninteractive' \
  'apt-get update -qq' \
  'apt-get install -y -qq dbus-x11 fonts-dejavu-core libwayland-client0 libx11-6 libxcursor1 libxi6 libxinerama1 libxkbcommon0 libxrandr2 xvfb >/dev/null' \
  'if dpkg-query -W libxkbcommon-x11-0 >/dev/null 2>&1; then echo "court image unexpectedly contains libxkbcommon-x11-0" >&2; exit 1; fi' \
  'if dpkg-query -W libxkbcommon-x11-dev >/dev/null 2>&1; then echo "court image unexpectedly contains libxkbcommon-x11-dev" >&2; exit 1; fi' \
  'ldconfig -p >/tmp/ldconfig.txt' \
  'if grep -F "libxkbcommon-x11.so.0" /tmp/ldconfig.txt >/dev/null; then echo "court image unexpectedly exposes libxkbcommon-x11.so.0" >&2; exit 1; fi' \
  'if grep -F "libxcb-xkb.so.1" /tmp/ldconfig.txt >/dev/null; then echo "court image unexpectedly exposes libxcb-xkb.so.1" >&2; exit 1; fi' \
  'unversioned=$(find /usr/lib /lib \( -type f -o -type l \) -name "libxkbcommon-x11.so" -print -quit 2>/dev/null)' \
  'if [ -n "$unversioned" ]; then echo "court image unexpectedly exposes $unversioned" >&2; exit 1; fi' \
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
  'if [ "$ready" -ne 1 ]; then child_rc=0; wait "$pid" || child_rc=$?; echo "MiniCon did not reach control readiness (exit=$child_rc)" >&2; cat /tmp/minicon.out >&2; cat /tmp/minicon.err >&2; find /root/.cache/minicon -maxdepth 3 -ls 2>/dev/null >&2 || true; exit 1; fi' \
  'grep -F '"'"'"active": "@1"'"'"' /tmp/snapshot.json >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Ctrl+Shift+I >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-ime commit "printf XKB_BUNDLED_OK" >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Enter >/dev/null' \
  '/court/minicon cli --control "$endpoint" ui-snapshot >/tmp/composer.json' \
  'grep -F '"'"'"composer_text": "printf XKB_BUNDLED_OK\n"'"'"' /tmp/composer.json >/dev/null' \
  '/court/minicon cli --control "$endpoint" send-ui-keys Ctrl+O >/dev/null' \
  '/court/minicon cli --control "$endpoint" wait-text --target @1 --timeout-ms 3000 XKB_BUNDLED_OK >/dev/null' \
  '/court/minicon cli --control "$endpoint" close-tab --target @1 >/dev/null' \
  '/court/minicon cli --control "$endpoint" ui-snapshot >/tmp/empty.json' \
  'grep -F '"'"'"workspace_empty": true'"'"' /tmp/empty.json >/dev/null' \
  '/court/minicon cli --control "$endpoint" close-window >/dev/null' \
  'wait "$pid"' \
  'if grep -E "panicked at|Linux X11 runtime dependenc|(/Users|/home)/[^/]+/[.]cargo" /tmp/minicon.err >/dev/null; then echo "runtime stderr violated the clean-package court:" >&2; cat /tmp/minicon.err >&2; exit 1; fi' >"$runtime_script"

docker run --rm \
  --volume "$PRODUCT_DIR:/court:ro" \
  --volume "$runtime_script:/runtime-court.sh:ro" \
  "$IMAGE" bash /runtime-court.sh

echo "linux X11 slim-desktop package court: PASS"
