#!/bin/bash
# Build-only check on real hosted-CI cells: dispatches dev-loop-crosscheck.yml
# with a test filter that matches nothing, so the workflow's own
# `cargo build --workspace --all-targets` step is the only thing that can
# fail it. Exists because this session's own environment cannot compile the
# product locally (see plan/plan-carried-debt.md's G1/G2 leaves for why
# dev-loop-crosscheck.yml itself exists) -- this script is the low-cost
# substitute: push to main, then run this instead of hand-driving
# `gh workflow run` + polling every time.
#
# Usage: scripts/ci-build.sh [cells] [ref]
#   cells  space-separated cell names (lnx-x86_64 lnx-aarch64 win-x86_64
#          win-aarch64 osx-aarch64 osx-x86_64), "all", or empty for the
#          workflow's own default (the four non-macOS cells). macOS cells
#          bill roughly 10x a Linux runner -- name them explicitly, don't
#          pass "all" out of habit.
#   ref    git ref to build; defaults to the current branch's upstream, or
#          "main" with no upstream.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
# shellcheck source=lib/ci-dispatch.sh
source scripts/lib/ci-dispatch.sh

cells=${1:-}
ref=${2:-$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null | sed 's#^origin/##' || true)}
ref=${ref:-main}

if ! minicon_ci_dispatch_and_wait dev-loop-crosscheck.yml "$ref" \
  "cells=$cells" 'test_filter=__ci_build_sh_matches_no_test__'; then
  echo "ci-build: FAILED -- see the run above for which cell's build broke" >&2
  exit 1
fi
echo "ci-build: build succeeded on every requested cell" >&2
