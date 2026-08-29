#!/bin/bash
# Bounded local build/test entry. Direct Cargo remains available, but this
# wrapper is the documented path because it reclaims stale local build state.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
mode=${1:-release}
shift || true

python3 scripts/cleanup-build-state.py --apply --scope routine
mkdir -p target
marker=target/.minicon-build-active
printf '%s\n' "$$" >"$marker"
cleanup() { rm -f "$marker"; }
trap cleanup EXIT HUP INT TERM

case "$mode" in
  release) cargo build --locked --release "$@" ;;
  dev) cargo build --locked "$@" ;;
  check) cargo check --locked --workspace --all-targets "$@" ;;
  test) cargo test --locked --workspace "$@" ;;
  *) echo "usage: scripts/build.sh [release|dev|check|test] [cargo arguments...]" >&2; exit 2 ;;
esac
