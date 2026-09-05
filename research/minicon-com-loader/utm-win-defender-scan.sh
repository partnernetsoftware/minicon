#!/bin/bash
# Internal exact-file-set Defender transport. Public wrappers establish evidence scope.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: utm-win-defender-scan.sh SCAN_MANIFEST FILE_DIR OUTPUT_RECEIPT" >&2
  exit 2
fi
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
COURT_CLI="$ROOT/scripts/utm-court.sh"
MANIFEST=$1
FILES=$2
OUTPUT=$3
COURT=win-x86_64-desktop
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
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  "del /f /q ${WINROOT}\\job.exit ${WINROOT}\\job.log ${WINROOT}\\job.ready ${WINROOT}\\job.pending.ps1 ${WINROOT}\\job.running.ps1"
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  "if exist ${WINROOT}\\defender rmdir /s /q ${WINROOT}\\defender & mkdir ${WINROOT}\\defender & mkdir ${WINROOT}\\defender\\files"
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c "start \"\" /min ${WINROOT}\\windows-utm-agent.cmd" || true
for file in "$FILES"/*; do
  leaf=$(basename "$file")
  remote=$(printf '%s\\defender\\files\\%s' "$WINROOT" "$leaf")
  "$COURT_CLI" push "$COURT" "$file" "$remote"
done
"$COURT_CLI" push "$COURT" "$MANIFEST" "$WINROOT\\defender\\candidate-manifest.json"
"$COURT_CLI" push "$COURT" "$HERE/defender-court.ps1" "$WINROOT\\job.pending.ps1"
printf ready | "$COURT_CLI" push "$COURT" - "$WINROOT\\job.ready"

deadline=$((SECONDS + 600))
while :; do
  : >"$tmp/exit"
  "$COURT_CLI" pull "$COURT" "$WINROOT\\job.exit" "$tmp/exit" 2>/dev/null || true
  [ -s "$tmp/exit" ] && break
  [ "$SECONDS" -lt "$deadline" ] || { echo "Defender court exceeded 10-minute deadline" >&2; exit 1; }
  sleep 2
done
"$COURT_CLI" pull "$COURT" "$WINROOT\\job.log" "$tmp/log" || true
cat "$tmp/log" 2>/dev/null || true
rc=$(tr -d '\r\n' <"$tmp/exit")
"$COURT_CLI" pull "$COURT" "$WINROOT\\defender\\defender-receipt.json" "$OUTPUT" || true
test -s "$OUTPUT" || { echo "Defender court produced no receipt (exit $rc)" >&2; exit 1; }
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
