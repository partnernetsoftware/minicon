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

mkdir -p "$FAKE/bin" "$FAKE/nested/deep"
printf 'old-cosmocc-sentinel\n' > "$FAKE/bin/cosmocc"
printf 'nested-sentinel\n' > "$FAKE/nested/deep/keep.txt"
chmod +x "$FAKE/bin/cosmocc"
OLD=$(shasum -a 256 "$FAKE/bin/cosmocc" | awk '{print $1}')
ln "$SRC_ZIP" "$FAKE/cosmocc-4.0.2.zip" 2>/dev/null || cp "$SRC_ZIP" "$FAKE/cosmocc-4.0.2.zip"
manifest() {
  (cd "$1" && find . -type f | sort)
}

set +e
COSMOCC_DIR="$FAKE" COSMOCC_FAIL_SWAP=1 bash "$HERE/install-cosmocc.sh"
rc=$?
set -e
if [[ "$rc" -ne 2 ]]; then
  echo "FAIL swap-test: want rc=2 got $rc" >&2
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
if [[ "$(cat "$FAKE/nested/deep/keep.txt")" != "nested-sentinel" ]]; then
  echo "FAIL swap-test: nested sentinel missing" >&2
  exit 1
fi
want=$(printf '%s\n' './bin/cosmocc' './cosmocc-4.0.2.zip' './nested/deep/keep.txt')
got=$(manifest "$FAKE")
if [[ "$got" != "$want" ]]; then
  echo "FAIL swap-test: tree manifest"$'\n'"$got" >&2
  exit 1
fi
if [[ -d "${FAKE}.lock" ]]; then
  echo "FAIL swap-test: lock dir leaked" >&2
  exit 1
fi
echo "PASS cosmocc-swap-mid-fail (rc=$rc digest=$OLD)"
