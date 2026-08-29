#!/bin/bash
# LaunchAgent entry: bound regenerable MiniCon build state once per day.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
exec /usr/bin/python3 scripts/cleanup-build-state.py --apply --scope all
