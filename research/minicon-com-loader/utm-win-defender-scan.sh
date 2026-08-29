#!/bin/bash
# Internal exact-file Defender transport. Public wrappers establish evidence scope.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: utm-win-defender-scan.sh SCAN_MANIFEST FILE OUTPUT_RECEIPT" >&2
  exit 2
fi

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
COURT_CLI="$ROOT/scripts/utm-court.sh"
MANIFEST=$1
SCAN_FILE=$2
OUTPUT=$3
COURT=win-x86_64-desktop
tmp=$(mktemp -d)
leased=0
cleanup() {
  rm -rf -- "$tmp"
  if [ "$leased" = 1 ]; then "$COURT_CLI" release "$COURT" >/dev/null || true; fi
}
trap cleanup EXIT HUP INT TERM

python3 - "$MANIFEST" "$SCAN_FILE" <<'PY'
import hashlib, json, pathlib, re, sys

manifest = json.load(open(sys.argv[1], encoding="utf-8"))
source = manifest.get("source_sha")
if not isinstance(source, str) or not re.fullmatch(r"[0-9a-f]{40}", source):
    raise SystemExit("scan manifest source_sha is invalid")
run = manifest.get("candidate_run")
if not isinstance(run, dict) or not isinstance(run.get("id"), int) or not isinstance(run.get("attempt"), int):
    raise SystemExit("scan manifest run identity is invalid")
scope = manifest.get("defender_evidence_scope")
if scope not in {"candidate", "diagnostic-probe"}:
    raise SystemExit("scan manifest evidence scope is invalid")
assets = [row for row in manifest.get("assets", []) if row.get("name") == "minicon.com"]
if len(assets) != 1:
    raise SystemExit("scan manifest must contain exactly one minicon.com")
actual = hashlib.sha256(pathlib.Path(sys.argv[2]).read_bytes()).hexdigest()
if assets[0].get("sha256") != actual:
    raise SystemExit("scan manifest/file digest mismatch")
PY

"$COURT_CLI" lease "$COURT" --disposable >/dev/null
leased=1
"$COURT_CLI" wait-ready "$COURT" 180 >/dev/null
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'del /f /q C:\minicon-six\job.exit C:\minicon-six\job.log C:\minicon-six\job.ready C:\minicon-six\job.pending.ps1 C:\minicon-six\job.running.ps1'
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'if not exist C:\minicon-six\defender mkdir C:\minicon-six\defender'
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'start "" /min C:\minicon-six\windows-utm-agent.cmd' || true
"$COURT_CLI" push "$COURT" "$SCAN_FILE" 'C:\minicon-six\defender\minicon.com'
"$COURT_CLI" push "$COURT" "$MANIFEST" 'C:\minicon-six\defender\candidate-manifest.json'
"$COURT_CLI" push "$COURT" "$HERE/defender-court.ps1" 'C:\minicon-six\job.pending.ps1'
printf ready | "$COURT_CLI" push "$COURT" - 'C:\minicon-six\job.ready'

deadline=$((SECONDS + 600))
while :; do
  : >"$tmp/exit"
  "$COURT_CLI" pull "$COURT" 'C:\minicon-six\job.exit' "$tmp/exit" 2>/dev/null || true
  [ -s "$tmp/exit" ] && break
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "Defender court exceeded 10-minute deadline" >&2
    exit 1
  fi
  sleep 2
done
"$COURT_CLI" pull "$COURT" 'C:\minicon-six\job.log' "$tmp/log" || true
cat "$tmp/log" 2>/dev/null || true
rc=$(tr -d '\r\n' <"$tmp/exit")
"$COURT_CLI" pull "$COURT" 'C:\minicon-six\defender\defender-receipt.json' "$OUTPUT" || true
test -s "$OUTPUT" || { echo "Defender court produced no receipt (exit $rc)" >&2; exit 1; }
python3 - "$MANIFEST" "$OUTPUT" "$SCAN_FILE" <<'PY'
import hashlib, json, pathlib, sys

manifest = json.load(open(sys.argv[1], encoding="utf-8"))
receipt = json.load(open(sys.argv[2], encoding="utf-8-sig"))
expected = hashlib.sha256(pathlib.Path(sys.argv[3]).read_bytes()).hexdigest()
assert receipt["kind"] == "minicon-defender-court"
assert receipt["evidence_scope"] == manifest["defender_evidence_scope"]
assert receipt["source_sha"] == manifest["source_sha"]
assert receipt["candidate_run"] == manifest["candidate_run"]
assert receipt["minicon_com_sha256"] == expected
for key in ("product_version", "engine_version", "signature_version", "scanned_at"):
    assert isinstance(receipt[key], str) and receipt[key]
print(f'{receipt["verdict"].upper()} UTM Defender {receipt["evidence_scope"]} receipt bound to exact SHA')
for row in receipt.get("threats", []):
    print(f'threat_id={row.get("threat_id")} threat_name={row.get("threat_name")}')
PY
test "$rc" = 0

