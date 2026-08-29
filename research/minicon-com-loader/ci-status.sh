#!/bin/bash
# Unix runner smoke: --version exact, argv passthrough (unknown flag ≠ 0), --status.
# Never compiles. GUI/control black-box is a later candidate gate, not this script.
set -euo pipefail
FAMILY="${1:?family linux|macos}"
BIN="${2:?path to minicon.com}"
RECEIPT="${3:-}"
case "$FAMILY" in linux|macos) ;; *) echo "family $FAMILY" >&2; exit 2 ;; esac
test -f "$BIN"
chmod +x "$BIN"

want=""
if [[ -n "$RECEIPT" && -f "$RECEIPT" ]]; then
  want=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["product_version"])' "$RECEIPT")
fi

ver="$("$BIN" --version 2>&1 | tr -d '\r' | sed -n '1p')"
echo "version_line=$ver"
if [[ -n "$want" ]]; then
  test "$ver" = "minicon $want"
fi

unkdir=$(mktemp -d)
set +e
"$BIN" --definitely-not-a-flag >"$unkdir/out" 2>"$unkdir/err"
unk=$?
set -e
echo "unknown_flag_exit=$unk"
[[ "$unk" -ne 0 ]]
grep -q "unknown argument" "$unkdir/err" "$unkdir/out"
rm -rf "$unkdir"

out="$("$BIN" --status 2>&1)" || { printf '%s\n' "$out"; exit 1; }
printf '%s\n' "$out"
printf '%s\n' "$out" | grep -q '^minicon '
printf '%s\n' "$out" | grep -q 'pty backend'
printf '%s\n' "$out" | grep -q 'unix-pty'
echo "PASS $FAMILY version+passthrough+status"
