#!/bin/bash
# Run exact host-linked Linux artifacts in a UTM QEMU Guest Agent court.

set -euo pipefail

[ "$#" -eq 3 ] || {
  echo "usage: scripts/linux-utm-runner.sh CELL TARGET_DIR status|test|throughput|stop" >&2
  exit 2
}
CELL="$1"; TARGET_DIR="$2"; MODE="$3"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${MINICON_REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
# shellcheck source=lib/utm-court.sh
. "$SCRIPT_DIR/lib/utm-court.sh"
COURT_CLI="$(minicon_utm_court_cli)" || exit 2

case "$CELL" in
  lnx-aarch64)
    COURT=lnx-aarch64-desktop
    VM="${MINICON_LINUX_UTM_AARCH64_VM:-minicon-lnx-arm-64}"
    ;;
  lnx-x86_64)
    COURT=lnx-x86_64-desktop
    VM="${MINICON_LINUX_UTM_X86_64_VM:-minicon-lnx-x86-64}"
    ;;
  *) echo "unsupported Linux cell: $CELL" >&2; exit 2 ;;
esac
case "$MODE" in status|test|throughput|stop) ;; *) exit 2 ;; esac
court() { UTM_COURT_VM="$VM" "$COURT_CLI" "$@"; }

if [ "$MODE" = stop ]; then
  court release "$COURT" >/dev/null
  exit $?
fi

profile=debug
[ "$MODE" = throughput ] && profile=release-fast
host_target="$REPO_ROOT/$TARGET_DIR"
[ -x "$host_target/$profile/minicon" ] || { echo "Linux artifact missing" >&2; exit 2; }
identity="$(printf '%s\n' "$TARGET_DIR" | sed -n 's#.*target-six/builds/\([0-9a-f]\{64\}\)/.*#\1#p')"
[ -n "$identity" ] || { echo "target directory lacks source fingerprint" >&2; exit 2; }

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/payload/target/$profile/deps"
cp -p "$host_target/$profile/minicon" "$scratch/payload/target/$profile/minicon"

# QGA is an artifact bridge, not a Cargo cache transport. Copy only the
# executable leaves owned by this mode; the former whole-profile copy moved
# almost two GiB of rlibs/metadata for a status check that needs one binary.
copy_test_prefix() {
  prefix="$1"; found=false
  for candidate in "$host_target/$profile/deps/$prefix"-*; do
    [ -x "$candidate" ] || continue
    # The unit-test harness is `minicon-<cargo-hash>` while every integration
    # harness starts `minicon_`. A broad `minicon-*` glob therefore defeats
    # payload narrowing unless the byte after the hyphen is validated.
    if [ "$prefix" = minicon ]; then
      case "${candidate##*/}" in minicon-[0-9a-f]*) ;; *) continue ;; esac
    fi
    cp -p "$candidate" "$scratch/payload/target/$profile/deps/"
    found=true
  done
  [ "$found" = true ] || { echo "Linux test artifact missing: $prefix" >&2; exit 2; }
}
case "$MODE" in
  status) ;;
  test)
    for prefix in minicon minicon_core minicon_alignment minicon_console_agent \
      minicon_load_portability minicon_control minicon_blackbox \
      minicon_accessibility_linux; do
      copy_test_prefix "$prefix"
    done
    ;;
  throughput) copy_test_prefix minicon_throughput ;;
esac
cp "$SCRIPT_DIR/linux-runtime-qualify.sh" "$scratch/payload/"
(cd "$scratch/payload" && tar -czf "$scratch/payload.tar.gz" .)
digest="$(shasum -a 256 "$scratch/payload.tar.gz" | awk '{print $1}')"
guest_root="/tmp/minicon-court-$identity-$MODE"

court lease "$COURT" --disposable >/dev/null
court wait-ready "$COURT" 180 >/dev/null
court push "$COURT" "$scratch/payload.tar.gz" "$guest_root.tar.gz"
court exec "$COURT" -- /bin/bash -lc \
  "set -e; test \"\$(sha256sum '$guest_root.tar.gz' | awk '{print \$1}')\" = '$digest'; rm -rf '$guest_root'; mkdir -p '$guest_root'; tar -xzf '$guest_root.tar.gz' -C '$guest_root'; chmod +x '$guest_root/linux-runtime-qualify.sh'; '$guest_root/linux-runtime-qualify.sh' '$guest_root/target' '$MODE'"
