#!/bin/bash
# Trampoline: MiniCon does not own the UTM court. See partnernetsoftware/utm-court.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/utm-court.sh
. "$SCRIPT_DIR/lib/utm-court.sh"
CLI="$(minicon_utm_court_cli)" || exit 2
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
export UTM_COURT_STATE_DIR="${UTM_COURT_STATE_DIR:-$REPO_ROOT/target-six/utm-court-service}"
export UTM_COURT_MACOS_BRIDGE="${UTM_COURT_MACOS_BRIDGE:-$REPO_ROOT/target-six/macos-utm-bridge}"
exec "$CLI" "$@"
