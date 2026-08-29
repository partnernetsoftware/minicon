#!/bin/bash
# Scan a sealed Candidate APE in the interactive Windows x86_64 UTM court.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: utm-win-defender-court.sh CANDIDATE_DIR OUTPUT_RECEIPT" >&2
  exit 2
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
COURT_CLI="$ROOT/scripts/utm-court.sh"
CANDIDATE="$1"
OUTPUT="$2"
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
python3 "$HERE/candidate_bundle.py" verify \
  --manifest "$CANDIDATE/candidate-manifest.json" --payload "$CANDIDATE/payload"

"$COURT_CLI" lease "$COURT" --disposable >/dev/null
leased=1
"$COURT_CLI" wait-ready "$COURT" 180 >/dev/null
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'del /f /q C:\minicon-six\job.exit C:\minicon-six\job.log C:\minicon-six\job.ready C:\minicon-six\job.pending.ps1 C:\minicon-six\job.running.ps1'
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'if not exist C:\minicon-six\defender mkdir C:\minicon-six\defender'
"$COURT_CLI" exec "$COURT" -- cmd.exe /d /c \
  'start "" /min C:\minicon-six\windows-utm-agent.cmd' || true
"$COURT_CLI" push "$COURT" "$CANDIDATE/payload/minicon.com" \
  'C:\minicon-six\defender\minicon.com'
"$COURT_CLI" push "$COURT" "$CANDIDATE/candidate-manifest.json" \
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
python3 - "$CANDIDATE/candidate-manifest.json" "$OUTPUT" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
r = json.load(open(sys.argv[2], encoding="utf-8-sig"))
a = [x for x in m["assets"] if x["name"] == "minicon.com"]
assert len(a) == 1
assert r["kind"] == "minicon-defender-court"
assert r["source_sha"] == m["source_sha"]
assert r["candidate_run"] == m["candidate_run"]
assert r["minicon_com_sha256"] == a[0]["sha256"]
for key in ("product_version", "engine_version", "signature_version", "scanned_at"):
    assert isinstance(r[key], str) and r[key]
print(f'{r["verdict"].upper()} UTM Defender receipt bound to exact Candidate')
if r["verdict"] != "clean":
    for row in r.get("threats", []):
        print(f'threat_id={row.get("threat_id")} threat_name={row.get("threat_name")}')
    raise SystemExit(1)
PY
test "$rc" = 0
