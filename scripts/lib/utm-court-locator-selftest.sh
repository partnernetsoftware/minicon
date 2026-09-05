#!/bin/bash
# Prove MiniCon locates utm-court without talking to utmctl.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=utm-court.sh
. "$SCRIPT_DIR/utm-court.sh"

cli="$(minicon_utm_court_cli)"
grep -q 'Uniform, product-neutral lifecycle' "$cli"
[ "$(basename "$cli")" = utm-court ]

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
printf '#!/bin/bash\nexit 0\n' >"$scratch/court"
chmod +x "$scratch/court"
found="$(MINICON_UTM_COURT_CLI="$scratch/court" minicon_utm_court_cli)"
[ "$found" = "$scratch/court" ]

"$SCRIPT_DIR/../utm-court.sh" help | grep -q 'windows-root'
"$SCRIPT_DIR/../utm-court.sh" windows-root | grep -q '\\'

echo "utm-court-locator-selftest: PASS"
