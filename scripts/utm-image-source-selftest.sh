#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
list="$($SCRIPT_DIR/utm-image-source.sh list)"
python3 - "$list" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
assert len(data["cells"]) == 6
assert {c["cell"] for c in data["cells"]} == {
    "osx-aarch64", "osx-x86_64", "lnx-aarch64",
    "lnx-x86_64", "win-aarch64", "win-x86_64",
}
PY

selection="$($SCRIPT_DIR/utm-image-source.sh select win-x86_64)"
python3 - "$selection" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
assert data["selection"] == "recipe-selected"
assert "ARM64-only" in data["reason"]
PY

set +e
missing="$($SCRIPT_DIR/utm-image-source.sh select unknown 2>/dev/null)"
status=$?
set -e
[ "$status" -eq 3 ]
python3 - "$missing" <<'PY'
import json, sys
assert json.loads(sys.argv[1])["selection"] == "no-qualified-image"
PY

echo "utm-image-source-selftest: PASS"
