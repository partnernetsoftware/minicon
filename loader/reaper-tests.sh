#!/bin/bash
# Four reaper cases: fake-symlink, foreign-owner, active-pid, stale-owned.
# Uses a host-cc probe of loader.c (same reap_stale_extracts). Does not spawn MiniCon.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PROBE="${TMPDIR:-/tmp}/minicon-reaper-probe.$$"
PROBE_UID="${TMPDIR:-/tmp}/minicon-reaper-probe-uid.$$"
WRAP="${TMPDIR:-/tmp}/minicon-reaper-geteuid.$$.c"
UIDN="$(id -u)"
fail=0
pass=0
declare -a leftovers=()

cleanup() {
  rm -f "$PROBE" "$PROBE_UID" "$WRAP" "${WRAP%.c}.o"
  local p
  for p in "${leftovers[@]+"${leftovers[@]}"}"; do
    if [[ -L "$p" ]]; then
      rm -f "$p"
    elif [[ -d "$p" ]]; then
      chmod -R u+w "$p" 2>/dev/null || true
      rm -rf "$p"
    elif [[ -e "$p" ]]; then
      rm -f "$p"
    fi
  done
}
trap cleanup EXIT

dead_pid() {
  local p=999999
  while kill -0 "$p" 2>/dev/null; do
    p=$((p - 1))
    if [[ "$p" -le 2 ]]; then
      echo "no dead pid" >&2
      return 1
    fi
  done
  echo "$p"
}

mark_dir() {
  local dir="$1"
  chmod 0700 "$dir"
  printf 'minicon.com-extract-v1 uid=%s\n' "$UIDN" > "$dir/.minicon-extract"
  chmod 0600 "$dir/.minicon-extract"
}

run_probe() {
  "$PROBE" >/dev/null 2>&1 || true
}

ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1" >&2; fail=$((fail + 1)); }

cc -O2 -o "$PROBE" "$HERE/loader.c"
cat > "$WRAP" <<'C'
#include <stdlib.h>
#include <unistd.h>
uid_t test_geteuid(void) {
    const char *e = getenv("MINICON_FAKE_EUID");
    if (e && *e) return (uid_t)atoi(e);
    return getuid();
}
C
cc -O2 -Dgeteuid=test_geteuid -o "$PROBE_UID" "$HERE/loader.c" "$WRAP"

DEAD="$(dead_pid)"
LIVEPID="$$"

# --- fake symlink: name match must not follow into victim ---
VICTIM=$(mktemp -d "${TMPDIR:-/tmp}/minicon-reaper-victim.XXXXXX")
echo SECRET > "$VICTIM/secret"
leftovers+=("$VICTIM")
FAKE="/tmp/minicon.com.${DEAD}.symlinktest"
ln -s "$VICTIM" "$FAKE"
leftovers+=("$FAKE")
run_probe
if [[ -L "$FAKE" && -f "$VICTIM/secret" && "$(cat "$VICTIM/secret")" == SECRET ]]; then
  ok fake-symlink
else
  bad fake-symlink
fi
rm -f "$FAKE"

# stale-owned dir whose child is a symlink must unlink the name, not the target
STALE_LNK="/tmp/minicon.com.${DEAD}.stalechild"
mkdir -m 0700 "$STALE_LNK"
mark_dir "$STALE_LNK"
ln -s "$VICTIM/secret" "$STALE_LNK/payload"
leftovers+=("$STALE_LNK")
run_probe
if [[ ! -e "$STALE_LNK" && -f "$VICTIM/secret" ]]; then
  ok fake-symlink-child
else
  bad fake-symlink-child
  rm -rf "$STALE_LNK" || true
fi

# --- foreign-owner: uid mismatch must not reap ---
FOREIGN="/tmp/minicon.com.${DEAD}.foreign"
mkdir -m 0700 "$FOREIGN"
mark_dir "$FOREIGN"
echo KEEP > "$FOREIGN/canary"
leftovers+=("$FOREIGN")
foreign_ok=0
if chown 65534 "$FOREIGN" 2>/dev/null; then
  run_probe
  if [[ -d "$FOREIGN" && -f "$FOREIGN/canary" ]]; then
    foreign_ok=1
  fi
  chown "$UIDN" "$FOREIGN" 2>/dev/null || true
else
  MINICON_FAKE_EUID=65534 "$PROBE_UID" >/dev/null 2>&1 || true
  if [[ -d "$FOREIGN" && -f "$FOREIGN/canary" ]]; then
    foreign_ok=1
  fi
fi
if [[ "$foreign_ok" -eq 1 ]]; then
  ok foreign-owner
else
  bad foreign-owner
fi
rm -rf "$FOREIGN"

# --- active-pid: live pid must not reap even with marker ---
ACTIVE="/tmp/minicon.com.${LIVEPID}.activetest"
mkdir -m 0700 "$ACTIVE"
mark_dir "$ACTIVE"
echo LIVE > "$ACTIVE/canary"
leftovers+=("$ACTIVE")
run_probe
if [[ -d "$ACTIVE" && -f "$ACTIVE/canary" ]]; then
  ok active-pid
else
  bad active-pid
fi
rm -rf "$ACTIVE"

# --- stale-owned: dead pid + 0700 + our uid + marker → reap ---
STALE="/tmp/minicon.com.${DEAD}.staleowned"
mkdir -m 0700 "$STALE"
mark_dir "$STALE"
echo GONE > "$STALE/canary"
leftovers+=("$STALE")
run_probe
if [[ ! -e "$STALE" ]]; then
  ok stale-owned
else
  bad stale-owned
  rm -rf "$STALE" || true
fi

echo "reaper-tests $pass passed, $fail failed"
exit "$fail"
