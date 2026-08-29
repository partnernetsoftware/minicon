#!/bin/bash
# Scan a sealed Candidate APE in the interactive Windows x86_64 UTM court.
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: utm-win-defender-court.sh CANDIDATE_DIR OUTPUT_RECEIPT [DIAGNOSTIC_FILE]" >&2
  exit 2
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
CANDIDATE="$1"
OUTPUT="$2"
SCAN_FILE="${3:-$CANDIDATE/payload/minicon.com}"
tmp="$(mktemp -d)"
cleanup() {
  rm -rf -- "$tmp"
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

"$HERE/utm-win-defender-scan.sh" "$tmp/scan-manifest.json" "$SCAN_FILE" "$OUTPUT"
