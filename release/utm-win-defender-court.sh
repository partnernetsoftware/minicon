#!/bin/bash
# Scan the release-policy-selected assets from one sealed Candidate.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: utm-win-defender-court.sh CANDIDATE_DIR OUTPUT_RECEIPT" >&2
  exit 2
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
CANDIDATE="$1"
OUTPUT="$2"
tmp="$(mktemp -d)"
cleanup() { rm -rf -- "$tmp"; }
trap cleanup EXIT

manifest="$CANDIDATE/candidate-manifest.json"
policy="$CANDIDATE/release-policy.json"
python3 "$HERE/candidate_bundle.py" verify --manifest "$manifest" \
  --payload "$CANDIDATE/payload" --policy "$policy"
mkdir -p "$tmp/files"
python3 - "$manifest" "$CANDIDATE/payload" "$tmp" <<'PY'
import hashlib, json, pathlib, sys, zipfile
manifest_path, payload, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
m = json.loads(manifest_path.read_text())
version = m["version"]
wanted = m["release_policy"]["reputation"]["assets"]
rows = {row["name"]: row for row in m["assets"]}
reputation = m["reputation_assets"]
spec = []
for key in wanted:
    if key == "minicon.com":
        name, leaf = key, "minicon.com"
        data = (payload / name).read_bytes()
    else:
        platform = {"windows-x86_64": "windows-x86_64", "windows-arm64": "windows-arm64"}[key]
        name, leaf = f"minicon-{version}-{platform}.zip", f"{key}.exe"
        with zipfile.ZipFile(payload / name) as archive:
            data = archive.read(f"minicon-{version}-{platform}/minicon.exe")
    digest = hashlib.sha256(data).hexdigest()
    if digest != reputation[key]["sha256"]:
        raise SystemExit(f"{key}: reputation digest mismatch")
    (out / "files" / leaf).write_bytes(data)
    spec.append({"key": key, "file": leaf, "sha256": digest})
m["defender_evidence_scope"] = "candidate"
m["defender_scan_assets"] = spec
(out / "scan-manifest.json").write_text(json.dumps(m, indent=2) + "\n")
PY
"$HERE/utm-win-defender-scan.sh" "$tmp/scan-manifest.json" "$tmp/files" "$OUTPUT"
