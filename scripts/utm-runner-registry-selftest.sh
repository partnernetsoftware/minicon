#!/bin/bash
# Keep product runner court IDs aligned with the shared UTM registry.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/utm-courts.json" \
  "$SCRIPT_DIR/linux-utm-runner.sh" "$SCRIPT_DIR/windows-utm-runner.sh" <<'PY'
import json
import sys
from pathlib import Path

registry_path, linux_path, windows_path = map(Path, sys.argv[1:])
registry = json.loads(registry_path.read_text(encoding="utf-8"))
ids = {court["id"] for court in registry["courts"]}
cells = {court["cell"] for court in registry["courts"]}
expected = {
    "lnx-aarch64-desktop": linux_path,
    "lnx-x86_64-desktop": linux_path,
    "win-aarch64-desktop": windows_path,
    "win-x86_64-desktop": windows_path,
}
for court_id, owner in expected.items():
    assert court_id in ids, f"registry missing {court_id}"
    source = owner.read_text(encoding="utf-8")
    assert f"COURT={court_id}" in source, f"{owner.name} missing {court_id}"

for court in registry["courts"]:
    if court["os"] == "win":
        assert court.get("interactive_user") == "minicon", f'{court["id"]} missing interactive user'
windows_source = windows_path.read_text(encoding="utf-8")
assert 'court interactive-ready "$COURT"' in windows_source, "Windows runner does not prove interactive agent readiness"

# OSX x86_64 is deliberately a host-Rosetta logical court on Apple Silicon,
# not a planned UTM asset. Keep the UTM inventory truthful and five-VM-only.
assert len(registry["courts"]) == 5, "UTM registry must contain exactly five VM courts"
assert "osx-x86_64" not in cells, "OSX x86_64 belongs to the host Rosetta court"
PY

echo "utm-runner-registry-selftest: PASS"
