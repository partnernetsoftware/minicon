#!/bin/bash
# Install/update MiniCon's per-user daily cleanup LaunchAgent.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
template="$repo_root/scripts/com.partnernetsoftware.minicon.cleanup.plist.in"
runner="$repo_root/scripts/macos-daily-cleanup.sh"
agents="$HOME/Library/LaunchAgents"
logs="$HOME/Library/Logs"
destination="$agents/com.partnernetsoftware.minicon.cleanup.plist"
log="$logs/minicon-maintenance.log"
domain="gui/$(id -u)"
label="$domain/com.partnernetsoftware.minicon.cleanup"

mkdir -p "$agents" "$logs"
python3 - "$template" "$destination" "$runner" "$log" <<'PY'
import plistlib
import sys
from pathlib import Path

template, destination, runner, log = map(Path, sys.argv[1:])
data = template.read_text(encoding="utf-8")
data = data.replace("__RUNNER__", str(runner)).replace("__LOG__", str(log))
parsed = plistlib.loads(data.encode("utf-8"))
Path(destination).write_bytes(plistlib.dumps(parsed, sort_keys=False))
PY

launchctl bootout "$label" >/dev/null 2>&1 || true
launchctl bootstrap "$domain" "$destination"
launchctl enable "$label"
launchctl print "$label" >/dev/null
printf 'installed=%s\nschedule=03:17 daily\nlog=%s\n' "$destination" "$log"
