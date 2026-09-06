#!/bin/bash
# Name MiniCon host RSS on osx, lnx and win from one Apple Silicon host.
#
# osx-aarch64 runs on the host first, then the same debug artifacts run in
# the clean macOS UTM guest. Linux and Windows execute exact host-linked
# artifacts in UTM. Guests are single-active: one lease at a time.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"
# shellcheck source=lib/utm-court.sh
. "$SCRIPT_DIR/lib/utm-court.sh"
COURT_CLI="$(minicon_utm_court_cli)" || exit 2

identity="$(python3 scripts/source-fingerprint.py |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["sha256"])')"
build_root="target-six/builds/$identity"
mkdir -p "$build_root" target-six/logs
receipt="target-six/logs/rss-os-court-$identity.jsonl"
: >"$receipt"

record_receipt() {
  local log="$1"
  local line
  line="$(grep MINICON_HOST_RSS_RECEIPT "$log" | tail -n 1 || true)"
  if [ -z "$line" ]; then
    echo "missing MINICON_HOST_RSS_RECEIPT in $log" >&2
    exit 1
  fi
  printf '%s\n' "$line" | tee -a "$receipt"
}

printf '[rss-os] identity %s\n' "$identity"
printf '[rss-os] court %s\n' "$COURT_CLI"

printf '[rss-os] osx-aarch64 host START\n'
CARGO_TARGET_DIR="$build_root/osx-aarch64" cargo test --locked \
  --test minicon_control --target aarch64-apple-darwin -- \
  --nocapture --test-threads=1 host_process_rss_stays_within_named_budget \
  2>&1 | tee "target-six/logs/rss-osx-aarch64-host.log"
record_receipt "target-six/logs/rss-osx-aarch64-host.log"
printf '[rss-os] osx-aarch64 host PASS\n'

printf '[rss-os] osx-aarch64 UTM START\n'
if "$SCRIPT_DIR/macos-utm-runner.sh" osx-aarch64 \
  "target-six/builds/$identity/osx-aarch64/aarch64-apple-darwin" rss \
  2>&1 | tee "target-six/logs/rss-osx-aarch64-utm.log"; then
  record_receipt "target-six/logs/rss-osx-aarch64-utm.log"
  printf '[rss-os] osx-aarch64 UTM PASS\n'
else
  printf '[rss-os] osx-aarch64 UTM BLOCKED (host receipt already named this OS)\n'
  printf 'MINICON_HOST_RSS_RECEIPT {"os":"macos","arch":"aarch64","court":"osx-aarch64-clean","status":"BLOCKED"}\n' |
    tee -a "$receipt"
fi
"$SCRIPT_DIR/macos-utm-runner.sh" osx-aarch64 . stop || true

printf '[rss-os] lnx-aarch64 link START\n'
CARGO_TARGET_DIR="$build_root/lnx-aarch64" cargo zigbuild --locked \
  --workspace --tests --target aarch64-unknown-linux-gnu
printf '[rss-os] lnx-aarch64 UTM START\n'
"$SCRIPT_DIR/linux-utm-runner.sh" lnx-aarch64 \
  "target-six/builds/$identity/lnx-aarch64/aarch64-unknown-linux-gnu" rss \
  2>&1 | tee "target-six/logs/rss-lnx-aarch64-utm.log"
record_receipt "target-six/logs/rss-lnx-aarch64-utm.log"
"$SCRIPT_DIR/linux-utm-runner.sh" lnx-aarch64 . stop || true
printf '[rss-os] lnx-aarch64 UTM PASS\n'

printf '[rss-os] win-aarch64 link START\n'
CARGO_TARGET_DIR="$build_root/win-aarch64" cargo xwin build --locked \
  --workspace --tests --target aarch64-pc-windows-msvc
printf '[rss-os] win-aarch64 UTM START\n'
"$SCRIPT_DIR/windows-utm-runner.sh" win-aarch64 \
  "target-six/builds/$identity/win-aarch64/aarch64-pc-windows-msvc" rss \
  2>&1 | tee "target-six/logs/rss-win-aarch64-utm.log"
record_receipt "target-six/logs/rss-win-aarch64-utm.log"
"$SCRIPT_DIR/windows-utm-runner.sh" win-aarch64 . stop || true
printf '[rss-os] win-aarch64 UTM PASS\n'

printf '[rss-os] receipts %s\n' "$receipt"
cat "$receipt"
