#!/bin/bash
# Inject a failure after DEST has been renamed aside; previous bin digest must remain.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC_ZIP="${COSMOCC_DIR:-$HOME/cosmocc}/cosmocc-${COSMOCC_VERSION:-4.0.2}.zip"
if [[ ! -s "$SRC_ZIP" ]]; then
  echo "need a verified cosmocc zip at $SRC_ZIP" >&2
  exit 2
fi
FAKE=$(mktemp -d "${TMPDIR:-/tmp}/cosmocc-swap-test.XXXXXX")
cleanup() { rm -rf "$FAKE" "${FAKE}.4.0.2.next."* "${FAKE}.4.0.2.prev."* 2>/dev/null || true; }
trap cleanup EXIT

mkdir -p "$FAKE/bin"
printf 'old-cosmocc-sentinel\n' > "$FAKE/bin/cosmocc"
chmod +x "$FAKE/bin/cosmocc"
OLD=$(shasum -a 256 "$FAKE/bin/cosmocc" | awk '{print $1}')
ln "$SRC_ZIP" "$FAKE/cosmocc-4.0.2.zip" 2>/dev/null || cp "$SRC_ZIP" "$FAKE/cosmocc-4.0.2.zip"

set +e
COSMOCC_DIR="$FAKE" COSMOCC_FAIL_SWAP=1 bash "$HERE/install-cosmocc.sh"
rc=$?
set -e
if [[ "$rc" -eq 0 ]]; then
  echo "FAIL swap-test: injected failure returned 0" >&2
  exit 1
fi
if [[ ! -x "$FAKE/bin/cosmocc" ]]; then
  echo "FAIL swap-test: live bin missing after rollback" >&2
  exit 1
fi
NEW=$(shasum -a 256 "$FAKE/bin/cosmocc" | awk '{print $1}')
if [[ "$NEW" != "$OLD" ]]; then
  echo "FAIL swap-test: bin digest changed: $OLD -> $NEW" >&2
  exit 1
fi
if [[ "$(cat "$FAKE/bin/cosmocc")" != "old-cosmocc-sentinel" ]]; then
  echo "FAIL swap-test: sentinel overwritten" >&2
  exit 1
fi
echo "PASS cosmocc-swap-mid-fail (rc=$rc digest=$OLD)"
