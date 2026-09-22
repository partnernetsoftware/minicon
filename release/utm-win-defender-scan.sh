#!/bin/bash
# Internal exact-file-set Defender transport. Public wrappers establish evidence scope.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: utm-win-defender-scan.sh SCAN_MANIFEST FILE_DIR OUTPUT_RECEIPT" >&2
  exit 2
fi
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
COURT_CLI="$ROOT/scripts/utm-court.sh"
MANIFEST=$1
FILES=$2
OUTPUT=$3
# Which Windows court runs the scan. Defaults to x86_64; overridable because
# Defender scans files without executing them, so a native Windows court of
# any ISA yields an equivalent verdict, and an emulated x86 guest may be
# unavailable on an Apple Silicon host.
COURT="${MINICON_DEFENDER_COURT:-win-x86_64-desktop}"
WINROOT="${UTM_COURT_WINDOWS_ROOT:-$("$COURT_CLI" windows-root)}"
tmp=$(mktemp -d)
leased=0
cleanup() {
  rm -rf -- "$tmp"
  if [ "$leased" = 1 ]; then "$COURT_CLI" release "$COURT" >/dev/null || true; fi
}
trap cleanup EXIT HUP INT TERM

python3 - "$MANIFEST" "$FILES" <<'PY'
import hashlib, json, pathlib, re, sys
m, root = json.load(open(sys.argv[1], encoding="utf-8")), pathlib.Path(sys.argv[2])
if not re.fullmatch(r"[0-9a-f]{40}", str(m.get("source_sha", ""))): raise SystemExit("invalid source_sha")
run = m.get("candidate_run")
if not isinstance(run, dict) or not isinstance(run.get("id"), int) or not isinstance(run.get("attempt"), int):
    raise SystemExit("invalid Candidate run")
assets = m.get("defender_scan_assets")
if not isinstance(assets, list) or not assets: raise SystemExit("no scan assets")
if len({row.get("key") for row in assets}) != len(assets): raise SystemExit("duplicate scan asset")
for row in assets:
    path = root / row["file"]
    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != row.get("sha256"):
        raise SystemExit(f"scan digest mismatch: {row.get('key')}")
PY

"$COURT_CLI" lease "$COURT" --disposable >/dev/null
leased=1
"$COURT_CLI" wait-ready "$COURT" 180 >/dev/null

# UTM 4.7's Apple-event transport intermittently errors a fresh disposable
# guest's first exec/push calls (OSStatus -10004/-2700) before the channel
# settles, which used to fail the whole court. Retry each control call and warm
# the channel first so a boot-time transport blip is ridden out, not fatal.
court_retry() {
  _n=0
  until "$COURT_CLI" "$@"; do
    _n=$((_n + 1))
    [ "$_n" -ge 10 ] && return 1
    sleep 3
  done
}
push_file() {
  _src="$1"; _dst="$2"
  _size=$(wc -c < "$_src" | tr -dc 0-9)
  if [ "$_size" -le 3145728 ]; then
    court_retry push "$COURT" "$_src" "$_dst"
    return
  fi
  # UTM 4.7's QGA file push refuses transfers over ~4 MiB ("being used by
  # another process"), independent of content. Split large files into <=3 MiB
  # parts, push each, concatenate in-guest. Byte-identical (the court's own
  # sha256 checks verify it); no Defender or exclusion changes. Paths here are
  # under C:\minicon-six with space-free leaf names, so the concat/del globs
  # run UNQUOTED: cmd's `copy /b` does not expand a "prefix"* glob where the
  # quote closes before the wildcard (it silently copies nothing), which is
  # what left minicon.com missing and failed the court.
  _cd=$(mktemp -d)
  ( cd "$_cd" && split -b 3145728 "$_src" "part." )
  _cerr=""
  case "$_dst$_src" in
    *[!A-Za-z0-9_./:\\-]*) _cerr="unsafe char in chunk path: $_dst" ;;
  esac
  [ -n "$_cerr" ] && { echo "$_cerr" >&2; rm -rf "$_cd"; return 1; }
  _ok=0
  _try=0
  while [ "$_try" -lt 5 ]; do
    _try=$((_try + 1))
    court_retry exec "$COURT" -- cmd.exe /d /c "del /f /q ${_dst} ${_dst}.part.* 2>nul & echo ok" >/dev/null
    for _part in "$_cd"/part.*; do
      court_retry push "$COURT" "$_part" "${_dst}.$(basename "$_part")"
    done
    court_retry exec "$COURT" -- cmd.exe /d /c "copy /b ${_dst}.part.* ${_dst} >nul & del /f /q ${_dst}.part.* & echo ok" >/dev/null
    # Verify the assembled file is byte-exact before trusting it: cmd's copy can
    # report success yet produce nothing. Pull the size the guest sees.
    court_retry exec "$COURT" -- cmd.exe /d /c "for %A in (${_dst}) do @echo %~zA> ${WINROOT}\\defender\\.csize" >/dev/null
    : >"$tmp/csize"
    court_retry pull "$COURT" "${WINROOT}\\defender\\.csize" "$tmp/csize" >/dev/null || true
    _got=$(tr -dc 0-9 <"$tmp/csize" 2>/dev/null)
    if [ "$_got" = "$_size" ]; then _ok=1; break; fi
    echo "chunk assemble mismatch for ${_dst}: got=${_got:-none} want=$_size (retry $_try)" >&2
    sleep 3
  done
  rm -rf "$_cd"
  [ "$_ok" = 1 ] || { echo "failed to assemble ${_dst} in guest" >&2; return 1; }
}

court_retry exec "$COURT" -- cmd.exe /d /c "echo warmup" >/dev/null

court_retry exec "$COURT" -- cmd.exe /d /c \
  "del /f /q ${WINROOT}\\job.exit ${WINROOT}\\job.log ${WINROOT}\\job.ready ${WINROOT}\\job.pending.ps1 ${WINROOT}\\job.running.ps1"
court_retry exec "$COURT" -- cmd.exe /d /c \
  "rmdir /s /q ${WINROOT}\\defender 2>nul & mkdir ${WINROOT}\\defender\\files & echo ok"
# exec output is not transported, so a silent mkdir cannot be trusted. Verify
# the transfer directory is really writable (a probe push lands) before sending
# the payload; retry mkdir + probe until it sticks.
printf x > "$tmp/dirprobe"
_dp=0
until court_retry push "$COURT" "$tmp/dirprobe" "${WINROOT}\\defender\\files\\.dirprobe"; do
  _dp=$((_dp + 1))
  [ "$_dp" -ge 10 ] && { echo "could not create a writable transfer directory" >&2; exit 1; }
  court_retry exec "$COURT" -- cmd.exe /d /c "mkdir ${WINROOT}\\defender\\files 2>nul & echo ok" >/dev/null
  sleep 2
done
for file in "$FILES"/*; do
  leaf=$(basename "$file")
  remote=$(printf '%s\\defender\\files\\%s' "$WINROOT" "$leaf")
  push_file "$file" "$remote"
done
push_file "$MANIFEST" "$WINROOT\\defender\\candidate-manifest.json"
push_file "$HERE/defender-court.ps1" "$WINROOT\\defender\\court.ps1"

# Run the scan SYNCHRONOUSLY. The async job-runner (windows-utm-agent.cmd) is
# not installed in an unsealed guest, and a detached `start /b` PowerShell is
# reaped by qemu-ga when the exec's cmd.exe returns, so it never finishes. A
# synchronous exec blocks until PowerShell exits, leaving the receipt on disk.
# The command taskkills any prior guest PowerShell first, so if UTM 4.7 drops
# the exec ack (OSStatus -10004/-2700) and court_retry re-fires, the orphaned
# run is killed and restarted cleanly instead of racing (this kills PowerShell
# inside the disposable Windows court VM, never anything on the host).
court_retry exec "$COURT" -- cmd.exe /d /c \
  "taskkill /f /im powershell.exe >nul 2>&1 & del /f /q ${WINROOT}\\defender\\defender-receipt.json 2>nul & powershell -NoProfile -ExecutionPolicy Bypass -File ${WINROOT}\\defender\\court.ps1 > ${WINROOT}\\defender\\court.log 2>&1 & echo done" >/dev/null
"$COURT_CLI" pull "$COURT" "$WINROOT\\defender\\court.log" "$tmp/log" 2>/dev/null || true
cat "$tmp/log" 2>/dev/null || true
# Pull the receipt (retry: the pull itself can hit a transport blip). Validate
# it parses as JSON so a partial read is retried, not accepted.
_rp=0
: >"$OUTPUT"
until "$COURT_CLI" pull "$COURT" "$WINROOT\\defender\\defender-receipt.json" "$OUTPUT" 2>/dev/null \
  && [ -s "$OUTPUT" ] \
  && python3 -c "import json,sys; json.load(open(sys.argv[1],encoding='utf-8-sig'))" "$OUTPUT" 2>/dev/null; do
  _rp=$((_rp + 1))
  [ "$_rp" -ge 10 ] && { echo "Defender court produced no receipt" >&2; exit 1; }
  : >"$OUTPUT"
  sleep 3
done
# exec's exit code is unreliable under UTM 4.7 (returns 0 even on guest error),
# so derive the gate from the receipt itself, which is the authoritative
# artifact the downstream assertions and reputation_court already validate.
rc=$(python3 -c "import json,sys; print(0 if json.load(open(sys.argv[1],encoding='utf-8-sig'))['verdict']=='clean' else 3)" "$OUTPUT")
python3 - "$MANIFEST" "$OUTPUT" <<'PY'
import json, sys
m = json.load(open(sys.argv[1], encoding="utf-8"))
r = json.load(open(sys.argv[2], encoding="utf-8-sig"))
expected = {row["key"]: row["sha256"] for row in m["defender_scan_assets"]}
assert r["schema"] == 2 and r["kind"] == "minicon-defender-court"
assert r["source_sha"] == m["source_sha"] and r["candidate_run"] == m["candidate_run"]
assert set(r["assets"]) == set(expected)
for key, digest in expected.items():
    assert r["assets"][key]["sha256"] == digest and r["assets"][key]["post_scan_sha256"] == digest
print(f'{r["verdict"].upper()} UTM Defender receipt assets={len(expected)}')
PY
test "$rc" = 0
