#!/bin/bash
# Host idle RSS/CPU for one already-linked tinygui cell.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cell="${1:-osx-aarch64}"
bin="$HERE/dist/$cell/tinygui"
[[ -x "$bin" ]] || bin="$HERE/dist/$cell/tinygui.exe"
[[ -e "$bin" ]] || {
  echo "missing $HERE/dist/$cell/tinygui; run build-six.sh" >&2
  exit 2
}
export TINYGUI_SETTLE_S="${SETTLE_S:-5}"
if [[ "$bin" == *.exe ]]; then
  echo "Windows guest measure is court-six.sh" >&2
  exit 2
fi
"$HERE/guest-unix.sh" "$bin"
