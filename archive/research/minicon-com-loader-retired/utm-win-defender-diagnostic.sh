#!/bin/bash
# Scan an exact non-Candidate APE without making Candidate/G6 claims.
set -euo pipefail

if [ "$#" -ne 5 ]; then
  echo "usage: utm-win-defender-diagnostic.sh FILE SOURCE_SHA RUN_ID RUN_ATTEMPT OUTPUT_RECEIPT" >&2
  exit 2
fi

HERE=$(cd "$(dirname "$0")" && pwd)
FILE=$1
SOURCE_SHA=$2
RUN_ID=$3
RUN_ATTEMPT=$4
OUTPUT=$5
tmp=$(mktemp -d)
cleanup() { rm -rf -- "$tmp"; }
trap cleanup EXIT HUP INT TERM

python3 - "$FILE" "$SOURCE_SHA" "$RUN_ID" "$RUN_ATTEMPT" "$tmp/manifest.json" <<'PY'
import hashlib, json, pathlib, re, sys

file, source, run_id, attempt, output = sys.argv[1:]
if not re.fullmatch(r"[0-9a-f]{40}", source):
    raise SystemExit("SOURCE_SHA must be 40 lowercase hex characters")
if not run_id.isdigit() or int(run_id) <= 0 or not attempt.isdigit() or int(attempt) <= 0:
    raise SystemExit("run identity must contain positive integers")
digest = hashlib.sha256(pathlib.Path(file).read_bytes()).hexdigest()
manifest = {
    "schema": 1,
    "kind": "minicon-defender-diagnostic",
    "defender_evidence_scope": "diagnostic-probe",
    "source_sha": source,
    "candidate_run": {"id": int(run_id), "attempt": int(attempt)},
    "assets": [{"name": "minicon.com", "sha256": digest}],
}
pathlib.Path(output).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY

"$HERE/utm-win-defender-scan.sh" "$tmp/manifest.json" "$FILE" "$OUTPUT"
