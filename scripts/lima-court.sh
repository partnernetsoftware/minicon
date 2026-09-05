#!/bin/bash
# Trampoline: MiniCon does not own the Lima court. See partnernetsoftware/utm-court.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/lima-court.sh
. "$SCRIPT_DIR/lib/lima-court.sh"
CLI="$(minicon_lima_court_cli)" || exit 2
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
export LIMA_COURT_STATE_DIR="${LIMA_COURT_STATE_DIR:-$REPO_ROOT/target-six/lima-court-service}"
exec "$CLI" "$@"
