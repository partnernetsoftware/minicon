#!/bin/zsh
# Capture a fresh full-window screenshot of MiniCon for the website.
#
# Run this from a real macOS GUI login session (a Terminal window you opened),
# NOT from a background/SSH/agent context — screencapture needs window-server
# access, and MiniCon needs an Aqua session to place its window on screen.
#
#   ./scripts/capture-website-screenshot.sh
#
# It launches a throwaway MiniCon, arranges a clean demo (three tabs, the Docs
# Ink theme, a little real output), captures its window to
# docs/assets/minicon-window.png, and closes the window cleanly. It never
# force-kills MiniCon; it asks the window to close through the control CLI.
set -u
cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
OUT="${1:-$ROOT/docs/assets/minicon-window.png}"

# Prefer a host release binary; fall back to the six-cell arm64 build; else build.
BIN=""
for cand in \
  "$ROOT/target/release/minicon" \
  "$ROOT/target-six/builds/current/osx-aarch64/aarch64-apple-darwin/release-fast/minicon" \
  "$ROOT/target-six/builds/current/osx-x86_64/x86_64-apple-darwin/release-fast/minicon"; do
  [ -x "$cand" ] && { BIN="$cand"; break; }
done
if [ -z "$BIN" ]; then
  echo "building target/release/minicon (first run only)…"
  cargo build --release >/dev/null || { echo "build failed" >&2; exit 1; }
  BIN="$ROOT/target/release/minicon"
fi
echo "binary: $BIN ($("$BIN" --version 2>/dev/null))"

# Private control socket (MiniCon refuses a world-accessible directory).
D="$(cd "${TMPDIR:-/tmp}" && pwd -P)/minicon-shot-$$"
mkdir -p "$D" && chmod 700 "$D"
SOCK="unix:$D/c.sock"
cleanup() { "$BIN" cli --control "$SOCK" close-window >/dev/null 2>&1; rm -rf "$D"; }
trap cleanup EXIT INT TERM

# Launch and drive from the caller's own GUI session so the window appears.
"$BIN" --control "$SOCK" --cols 118 --rows 30 -e /bin/zsh &
GUI=$!
ctl() { "$BIN" cli --control "$SOCK" "$@"; }

# Wait for the control endpoint.
for i in $(seq 1 60); do ctl list-tabs >/dev/null 2>&1 && break; sleep 0.5; done
ctl list-tabs >/dev/null 2>&1 || { echo "control endpoint never came up" >&2; exit 1; }

# A crisp 1200x720 window (matches the site's ~1.68 aspect).
ctl resize-window --width 1200 --height 720 >/dev/null 2>&1

# Docs Ink theme (Neutral -> Docs is one press of Ctrl+Shift+P) for a branded look.
ctl send-ui-keys Ctrl+Shift+P >/dev/null 2>&1

# A small tab tree so the column is not empty.
ctl new-tab >/dev/null 2>&1
ctl new-tab >/dev/null 2>&1
sleep 0.5

# A little believable output in the first tab, then leave it active.
ctl send-text --target @1 "uname -sm; sw_vers -productName 2>/dev/null" >/dev/null 2>&1
ctl send-keys --target @1 Enter >/dev/null 2>&1 || ctl send-text --target @1 $'\r' >/dev/null 2>&1
sleep 0.4
ctl send-text --target @1 "git -C \"$ROOT\" log --oneline -4" >/dev/null 2>&1
ctl send-text --target @1 $'\r' >/dev/null 2>&1
# Focus the first tab so it is the visible one.
ctl select-tab --target @1 >/dev/null 2>&1 || true
sleep 1.2

# Find MiniCon's on-screen window id by pid and capture just that window.
WID="$(/usr/bin/swift - "$GUI" <<'SW' 2>/dev/null
import CoreGraphics
import Foundation
let pid = Int32(CommandLine.arguments[1]) ?? -1
if let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
  for w in list {
    if let owner = w[kCGWindowOwnerPID as String] as? Int32, owner == pid,
       let num = w[kCGWindowNumber as String] as? Int {
      print(num); break
    }
  }
}
SW
)"

if [ -n "$WID" ]; then
  /usr/sbin/screencapture -o -x -l"$WID" "$OUT" && echo "captured window -> $OUT"
else
  echo "could not resolve the MiniCon window id; capturing interactively." >&2
  echo "Click the MiniCon window when the crosshair appears…" >&2
  /usr/sbin/screencapture -o -w "$OUT" && echo "captured -> $OUT"
fi

# Report the result; cleanup() closes the window on exit.
if [ -s "$OUT" ]; then
  echo "size: $(wc -c < "$OUT") bytes"
  /usr/bin/sips -g pixelWidth -g pixelHeight "$OUT" 2>/dev/null | grep pixel || true
fi
