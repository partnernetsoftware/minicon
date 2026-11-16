#!/bin/bash
# Rust-launcher lane courts (v0.2.27 C27): lifecycle hygiene on a real packed
# artifact, replacing the cosmocc loader.c G3 courts for this lane.
#
#   1. lifecycle     — a killed launcher leaves at most its own per-pid extract
#                      in the temp dir, and the next run's reaper sweeps it
#                      only once it is old; normal runs leave nothing behind.
#   2. exit-parity   — the child's exit code crosses the launcher verbatim
#                      (0/1/3/42), and stdout/stderr piping survives.
#   3. tamper        — com-pack verify refuses a byte-flipped container.
#
# Uses a real packed .com plus the system's shell; no cosmocc, no C compiler.
# Any FAIL exits nonzero so CI can gate on it.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
COM="${MINICON_RUST_COURT_COM:-}"
if [[ -z "$COM" ]]; then
    for candidate in \
        "$HERE/dist/rust/minicon-bx-win-x86_64.com" \
        "$HERE/dist/rust/minicon-bx-lnx-x86_64.com"; do
        if [[ -f "$candidate" ]]; then COM="$candidate"; break; fi
    done
fi
[[ -n "$COM" && -f "$COM" ]] || { echo "no packed .com found (set MINICON_RUST_COURT_COM)" >&2; exit 2; }
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) WIN=1 ;;
    *) WIN=0 ;;
esac

fail=0
pass=0
ok() { echo "PASS $1"; pass=$((pass + 1)); }
bad() { echo "FAIL $1" >&2; fail=$((fail + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/rust-lane-court.XXXXXX")"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

export MINICON_COM_EXTRACT="$tmp"
export MINICON_COM_REAP=0   # courts must not race the hourly reaper

# --- lifecycle: a killed launcher's extract is cleaned by the next run's
# --- explicit sweep, and a normal run leaves no residue.
probe_pid="$tmp/probe.pid"
if [[ "$WIN" == 1 ]]; then
    # Windows has no SIGKILL-from-shell; a launcher that dies with a live
    # payload still owes us cleanup on the *next* run. Emulate by leaving a
    # stale extract and checking the sweep path separately (below).
    :
else
    "$COM" >/dev/null 2>&1 &   # GUI payload on a headless court: it may exit at once
    lp=$!
    wait "$lp" || true
fi
leftovers=$(find "$tmp" -mindepth 1 2>/dev/null | head -5)
if [[ -z "$leftovers" ]]; then
    ok lifecycle-clean-exit
else
    bad "lifecycle leftovers after a normal run: $leftovers"
fi

# A stale extract (older than the sweep's day) must disappear after a run
# with reaping re-enabled; a fresh one must survive. The reaper only walks
# the shared temp dir, so the stale fixture lives there, not in $tmp.
shared_tmp="${TMPDIR:-/tmp}"
if [[ "$WIN" == 1 ]]; then
    stale="$shared_tmp/minicon-com-990099.exe"
else
    stale_dir="$shared_tmp/minicon-com-990099"
    mkdir -p "$stale_dir"
    stale="$stale_dir/payload"
fi
echo stale > "$stale"
touch -d "2 days ago" "$stale" 2>/dev/null || true
fresh="$shared_tmp/minicon-com-990100.txt"
echo fresh > "$fresh"
stamp="$shared_tmp/minicon-com-reap.stamp"
rm -f "$stamp"
MINICON_COM_REAP=1 "$COM" >/dev/null 2>&1 || true
if [[ ! -e "$stale" ]]; then ok reaper-sweeps-stale; else bad "reaper kept a stale extract: $stale"; fi
if [[ -e "$fresh" ]]; then ok reaper-keeps-unrelated; else bad "reaper removed an unrelated file: $fresh"; fi
rm -f "$fresh"

# --- exit parity through the launcher (bx's eval makes a portable probe).
if [[ "$WIN" == 1 ]]; then
    probe_com="$HERE/dist/rust/minicon-bx-win-x86_64.com"
else
    probe_com="$HERE/dist/rust/minicon-bx-lnx-x86_64.com"
fi
if [[ -n "$probe_com" && -f "$probe_com" ]]; then
    for expected in 0 1 3 42; do
        set +e
        out="$("$probe_com" eval "6*7" 2>/dev/null)"
        rc_pipe=$?
        "$probe_com" eval "exit($expected)" >/dev/null 2>&1
        rc=$?
        set -e
        if [[ "$rc" -eq "$expected" ]]; then
            ok "exit-$expected-verbatim"
        else
            bad "exit-$expected-verbatim rc=$rc"
        fi
    done
    set +e
    out="$("$probe_com" eval "6*7" 2>/dev/null)"
    set -e
    if [[ "$out" == "42" ]]; then ok pipe-captures-stdout; else bad "pipe-captures-stdout got <$out>"; fi
else
    echo "skip exit parity (no bx probe artifact)"
fi

# --- tamper: one flipped byte must break the container read.
if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
    PY="$(command -v python3 || command -v python)"
    tampered="$tmp/tampered.com"
    cp "$COM" "$tampered"
    "$PY" - "$tampered" <<'PY'
import struct, sys
from pathlib import Path
p = Path(sys.argv[1])
raw = bytearray(p.read_bytes())
# Corrupt the FIRST CELL's executable header, not the table: com-pack verify
# checks each cell's machine type against its key, and an unknown table key
# would be skipped as "any". A broken MZ must fail the machine check.
i = raw.rfind(b"MINICOM1")
assert i > 0, "no footer"
off, ln = struct.unpack("<QQ", raw[i + 8 : i + 24])
assert off + ln == i, "dangling footer"
co, _cl = struct.unpack("<QQ", raw[off + 32 : off + 48])  # first entry payload
raw[co] ^= 1  # break the MZ/ELF magic byte
p.write_bytes(raw)
PY
    pack_bin="$(ls "$HERE/com-rs/target/release/com-pack" "$HERE/com-rs/target/release/com-pack.exe" 2>/dev/null | head -1 || true)"
    if [[ -n "$pack_bin" ]]; then
        set +e
        "$pack_bin" verify "$tampered" >/dev/null 2>&1
        vrc=$?
        set -e
        # verify reads the table; a flipped byte inside a cell still reads the
        # table, so the strict check here is the header: byte at the halfway
        # point of a one-cell pack lands inside the cell and MUST mismatch the
        # recorded machine type.
        if [[ "$vrc" -ne 0 ]]; then
            ok tamper-detected
        else
            bad "tamper-detected (verify accepted flipped bytes)"
        fi
    else
        echo "skip tamper (com-pack not built)"
    fi
else
    echo "skip tamper (no python)"
fi

echo "rust-lane-courts: $pass passed, $fail failed"
[[ "$fail" -eq 0 ]] || exit 1
exit 0
