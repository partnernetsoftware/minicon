#!/bin/bash
# G3: SIGKILL cleans extract; non-EINTR waitpid failure keeps extract.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TMP="${TMPDIR:-/tmp}"
SLEEPBIN="$TMP/minicon-lifecycle-sleep.$$"
CELLS="$TMP/minicon-lifecycle-cells.$$"
PROBE="$TMP/minicon-lifecycle-probe.$$"
PROBE_W="$TMP/minicon-lifecycle-waitpid.$$"
WRAP="$TMP/minicon-lifecycle-waitpid.$$.c"
fail=0
pass=0

cleanup() {
  pkill -KILL -P $$ 2>/dev/null || true
  rm -rf "$SLEEPBIN" "$CELLS" "$PROBE" "$PROBE_W" "$WRAP"
  rm -rf /tmp/minicon.com.$$.* 2>/dev/null || true
}
trap cleanup EXIT

ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1" >&2; fail=$((fail + 1)); }

cc -O2 -o "$SLEEPBIN" -x c - <<'C'
#include <unistd.h>
int main(void) { for (;;) pause(); }
C
mkdir -p "$CELLS/osx-aarch64" "$CELLS/osx-x86_64" "$CELLS/lnx-aarch64" "$CELLS/lnx-x86_64"
cp "$SLEEPBIN" "$CELLS/osx-aarch64/minicon"
cp "$SLEEPBIN" "$CELLS/osx-x86_64/minicon"
cp "$SLEEPBIN" "$CELLS/lnx-aarch64/minicon"
cp "$SLEEPBIN" "$CELLS/lnx-x86_64/minicon"
chmod +x "$CELLS"/*/minicon

cc -O2 -o "$PROBE" "$HERE/loader.c"
cat > "$WRAP" <<'C'
#include <errno.h>
#include <stdlib.h>
#include <sys/types.h>
#include <sys/wait.h>
pid_t test_waitpid(pid_t pid, int *status, int options) {
    const char *e = getenv("MINICON_WAITPID_FAIL");
    if (e && *e) {
        errno = EIO;
        return -1;
    }
    return wait4(pid, status, options, 0);
}
C
cc -O2 -Dwaitpid=test_waitpid -o "$PROBE_W" "$HERE/loader.c" "$WRAP"

extracts_for() {
  local pid="$1" p out=""
  shopt -s nullglob
  for p in /tmp/minicon.com."${pid}".* /private/tmp/minicon.com."${pid}".*; do
    [[ -e "$p" ]] || continue
    out="$out$p"$'\n'
  done
  printf '%s' "$out"
}

# T-sigkill: waitpid succeeds (signaled), loader cleans, exit 128+KILL
MINICON_COM_CELLS="$CELLS" "$PROBE" >/dev/null 2>&1 &
lp=$!
for _ in $(seq 1 50); do
  maps=$(extracts_for "$lp")
  [[ -n "$maps" ]] && break
  sleep 0.05
done
child=$(pgrep -P "$lp" | awk 'NR==1')
if [[ -z "$child" ]]; then
  bad sigkill-no-child
else
  kill -KILL "$child" 2>/dev/null || true
  set +e
  wait "$lp"
  rc=$?
  set -e
  left=$(extracts_for "$lp")
  if [[ "$rc" -eq $((128 + 9)) && -z "$left" ]]; then
    ok sigkill-cleans
  else
    bad "sigkill-cleans rc=$rc leftover=$left"
    rm -rf $left
  fi
fi

# T-waitpid-fail: non-EINTR waitpid error → keep dir, exit 6
MINICON_WAITPID_FAIL=1 MINICON_COM_CELLS="$CELLS" "$PROBE_W" >/dev/null 2>&1 &
wp=$!
for _ in $(seq 1 50); do
  if ! kill -0 "$wp" 2>/dev/null; then break; fi
  sleep 0.05
done
set +e
wait "$wp"
wrc=$?
set -e
kept=$(extracts_for "$wp")
# child of failed-wait loader may still run under the extract binary
if [[ "$wrc" -eq 6 && -n "$kept" ]]; then
  ok waitpid-fail-keeps
else
  bad "waitpid-fail-keeps rc=$wrc kept=$kept"
fi
if [[ -n "$kept" ]]; then
  # stop any payload still using the extract, then drop leftover
  pkill -KILL -f "$kept" 2>/dev/null || true
  rm -rf $kept
fi

# Two loaders at once: two private extract dirs
MINICON_COM_CELLS="$CELLS" "$PROBE" >/dev/null 2>&1 &
a=$!
MINICON_COM_CELLS="$CELLS" "$PROBE" >/dev/null 2>&1 &
b=$!
for _ in $(seq 1 50); do
  da=$(extracts_for "$a")
  db=$(extracts_for "$b")
  [[ -n "$da" && -n "$db" ]] && break
  sleep 0.05
done
da=$(extracts_for "$a")
db=$(extracts_for "$b")
if [[ -n "$da" && -n "$db" && "$da" != "$db" ]]; then
  ok concurrent-extracts
else
  bad "concurrent-extracts a=$da b=$db"
fi
kill -KILL "$a" "$b" 2>/dev/null || true
wait "$a" 2>/dev/null || true
wait "$b" 2>/dev/null || true
pkill -KILL -f "$CELLS" 2>/dev/null || true
rm -rf $da $db 2>/dev/null || true

echo "lifecycle-tests $pass passed, $fail failed"
exit "$fail"
