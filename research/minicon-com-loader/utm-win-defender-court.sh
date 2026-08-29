#!/bin/bash
# Scan a sealed Candidate APE in the interactive Windows x86_64 UTM court.
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: utm-win-defender-court.sh CANDIDATE_DIR OUTPUT_RECEIPT [DIAGNOSTIC_FILE]" >&2
  exit 2
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
COURT_CLI="$ROOT/scripts/utm-court.sh"
CANDIDATE="$1"
OUTPUT="$2"
SCAN_FILE="${3:-$CANDIDATE/payload/minicon.com}"
COURT=win-x86_64-desktop
tmp="$(mktemp -d)"
leased=0
cleanup() {
  rm -rf "$tmp"
  if [ "$leased" = 1 ]; then "$COURT_CLI" release "$COURT" >/dev/null || true; fi
}
trap cleanup EXIT

test -f "$CANDIDATE/candidate-manifest.json"
test -f "$CANDIDATE/payload/minicon.com"
test -f "$SCAN_FILE"
python3 "$HERE/candidate_bundle.py" verify \
  --manifest "$CANDIDATE/candidate-manifest.json" --payload "$CANDIDATE/payload"
cp "$CANDIDATE/candidate-manifest.json" "$tmp/scan-manifest.json"
python3 - "$tmp/scan-manifest.json" "$SCAN_FILE" "${3:+diagnostic-probe}" <<'PY'
import hashlib, json, pathlib, sys
p = pathlib.Path(sys.argv[1])
m = json.loads(p.read_text())
sha = hashlib.sha256(pathlib.Path(sys.argv[2]).read_bytes()).hexdigest()
row = next(x for x in m["assets"] if x["name"] == "minicon.com")
row["sha256"] = sha
m["defender_evidence_scope"] = sys.argv[3] or "candidate"
p.write_text(json.dumps(m, indent=2) + "\n")
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
"$COURT_CLI" push "$COURT" "$SCAN_FILE" \
  'C:\minicon-six\defender\minicon.com'
"$COURT_CLI" push "$COURT" "$tmp/scan-manifest.json" \
  'C:\minicon-six\defender\candidate-manifest.json'
"$COURT_CLI" push "$COURT" "$HERE/defender-court.ps1" \
  'C:\minicon-six\job.pending.ps1'
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
rc="$(tr -d '\r\n' <"$tmp/exit")"
"$COURT_CLI" pull "$COURT" 'C:\minicon-six\defender\defender-receipt.json' "$OUTPUT" || true
test -s "$OUTPUT" || { echo "Defender court produced no receipt (exit $rc)" >&2; exit 1; }
python3 - "$CANDIDATE/candidate-manifest.json" "$OUTPUT" "$SCAN_FILE" <<'PY'
import hashlib, json, pathlib, sys
m = json.load(open(sys.argv[1]))
r = json.load(open(sys.argv[2], encoding="utf-8-sig"))
a = [x for x in m["assets"] if x["name"] == "minicon.com"]
assert len(a) == 1
assert r["kind"] == "minicon-defender-court"
assert r["source_sha"] == m["source_sha"]
assert r["candidate_run"] == m["candidate_run"]
expected = hashlib.sha256(pathlib.Path(sys.argv[3]).read_bytes()).hexdigest()
assert r["minicon_com_sha256"] == expected
for key in ("product_version", "engine_version", "signature_version", "scanned_at"):
    assert isinstance(r[key], str) and r[key]
print(f'{r["verdict"].upper()} UTM Defender receipt bound to exact Candidate')
if r["verdict"] != "clean":
    for row in r.get("threats", []):
        print(f'threat_id={row.get("threat_id")} threat_name={row.get("threat_name")}')
    raise SystemExit(1)
PY
test "$rc" = 0
