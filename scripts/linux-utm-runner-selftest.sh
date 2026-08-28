#!/bin/bash
# Prove Linux UTM payload ownership without starting a virtual machine.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

identity=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
target_rel="target-six/builds/$identity/lnx-x86_64"
target="$scratch/repo/$target_rel/debug"
mkdir -p "$target/deps"

make_executable() {
  printf '#!/bin/sh\nexit 0\n' >"$1"
  chmod +x "$1"
}

make_executable "$target/minicon"
for name in minicon minicon_core minicon_alignment minicon_console_agent \
  minicon_load_portability minicon_control minicon_blackbox \
  minicon_accessibility_linux minicon_throughput minicon_unowned; do
  make_executable "$target/deps/$name-abcdef"
done

cat >"$scratch/court" <<'SH'
#!/bin/bash
set -euo pipefail
case "$1" in
  lease|wait-ready|exec) exit 0 ;;
  push)
    tar -tzf "$3" | LC_ALL=C sort >"$PAYLOAD_LIST"
    ;;
  *) exit 2 ;;
esac
SH
chmod +x "$scratch/court"

PAYLOAD_LIST="$scratch/status.list" MINICON_REPO_ROOT="$scratch/repo" \
  MINICON_UTM_COURT_CLI="$scratch/court" \
  "$SCRIPT_DIR/linux-utm-runner.sh" lnx-x86_64 "$target_rel" status
grep -qx './target/debug/minicon' "$scratch/status.list"
if grep -Eq '/deps/[^/]+' "$scratch/status.list"; then
  echo "status payload unexpectedly contains test executables" >&2
  exit 1
fi

PAYLOAD_LIST="$scratch/test.list" MINICON_REPO_ROOT="$scratch/repo" \
  MINICON_UTM_COURT_CLI="$scratch/court" \
  "$SCRIPT_DIR/linux-utm-runner.sh" lnx-x86_64 "$target_rel" test
for name in minicon minicon_core minicon_alignment minicon_console_agent \
  minicon_load_portability minicon_control minicon_blackbox \
  minicon_accessibility_linux; do
  grep -qx "./target/debug/deps/$name-abcdef" "$scratch/test.list"
done
if grep -Eq 'minicon_(throughput|unowned)-' "$scratch/test.list"; then
  echo "test payload contains an executable not owned by test mode" >&2
  exit 1
fi

echo "linux-utm-runner-selftest: PASS"
