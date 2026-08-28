#!/bin/bash
# Interactive launcher for preparing the reusable Ubuntu desktop UTM media.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$SCRIPT_DIR/prepare-linux-desktop-utm.sh"
