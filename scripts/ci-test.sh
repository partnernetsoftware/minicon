#!/bin/bash
# Build + run the real test suites on real hosted-CI cells, via
# dev-loop-crosscheck.yml. This is the same workflow scripts/ci-build.sh
# uses, minus the no-op test filter -- so it also proves the build, but the
# thing worth reading afterward is the suite results, not just the build
# step.
#
# Usage: scripts/ci-test.sh [cells] [ref] [test_filter]
#   cells        space-separated cell names (lnx-x86_64 lnx-aarch64
#                win-x86_64 win-aarch64 osx-aarch64 osx-x86_64), "all", or
#                empty for the workflow's own default (the four non-macOS
#                cells). macOS cells bill roughly 10x a Linux runner -- name
#                them explicitly, don't pass "all" out of habit.
#   ref          git ref to build and test; defaults to the current
#                branch's upstream, or "main" with no upstream.
#   test_filter  Rust test-name substring, applied to every suite; empty
#                runs everything.
#
# On failure this also dumps the failed job's log (gh run view --log-failed)
# so the caller doesn't have to open the Actions UI to see which test broke.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
# shellcheck source=lib/ci-dispatch.sh
source scripts/lib/ci-dispatch.sh

cells=${1:-}
ref=${2:-$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null | sed 's#^origin/##' || true)}
ref=${ref:-main}
test_filter=${3:-}

repo=$(_ci_dispatch_repo)
since_marker=$(date -u +%Y-%m-%dT%H:%M:%SZ)

if minicon_ci_dispatch_and_wait dev-loop-crosscheck.yml "$ref" \
  "cells=$cells" "test_filter=$test_filter"; then
  echo "ci-test: every requested suite passed on every requested cell" >&2
  exit 0
fi

run_id=$(gh run list --repo "$repo" --workflow dev-loop-crosscheck.yml \
  --created "$since_marker..*" --json databaseId,createdAt \
  --jq 'sort_by(.createdAt) | last | .databaseId // empty')
echo "ci-test: FAILED -- failed step output follows" >&2
if [ -n "$run_id" ]; then
  minicon_ci_dump_failed_logs "$run_id"
fi
exit 1
