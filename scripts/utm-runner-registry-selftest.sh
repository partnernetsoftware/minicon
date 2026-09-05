#!/bin/bash
# Keep MiniCon product runners aligned with court IDs. Court internals live
# in partnernetsoftware/utm-court.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/linux-utm-runner.sh" "$SCRIPT_DIR/windows-utm-runner.sh" \
  "$SCRIPT_DIR/macos-utm-runner.sh" <<'PY'
import sys
from pathlib import Path

linux_path, windows_path, macos_path = map(Path, sys.argv[1:])
linux_source = linux_path.read_text(encoding="utf-8")
windows_source = windows_path.read_text(encoding="utf-8")
macos_source = macos_path.read_text(encoding="utf-8")
for court_id in ("lnx-aarch64-desktop", "lnx-x86_64-desktop"):
    assert f"COURT={court_id}" in linux_source, f"{linux_path.name} missing {court_id}"
for court_id in ("win-aarch64-desktop", "win-x86_64-desktop"):
    assert f"COURT={court_id}" in windows_source, f"{windows_path.name} missing {court_id}"
assert "court lease osx-aarch64" in macos_source or "osx-aarch64" in macos_source
assert 'court interactive-ready "$COURT" 180' in windows_source
assert "agent-v2" in windows_source
assert "utmctl" not in linux_source, "Linux runner still talks to utmctl"
assert "utmctl" not in windows_source, "Windows runner still talks to utmctl"
assert r"C:\\minicon-six" not in windows_source, "Windows runner hardcodes the guest root"
assert "windows-root" in windows_source
assert "lib/utm-court.sh" in linux_source
assert "lib/utm-court.sh" in windows_source
assert "lib/utm-court.sh" in macos_source
assert "prepare-macos" in macos_source
assert "hdiutil" not in macos_source, "macOS runner still builds bootstrap media"
PY

"$SCRIPT_DIR/lib/utm-court-locator-selftest.sh"
echo "utm-runner-registry-selftest: PASS"
