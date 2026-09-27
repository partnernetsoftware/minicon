#!/bin/bash
# Thin `gh workflow run` wrappers for the three release-chain stages
# (Candidate -> Reputation -> Release), so driving them by hand doesn't mean
# retyping each workflow's `workflow_dispatch` input names from memory.
#
# This script does NOT decide policy and must not be extended to: it only
# fills in the inputs each workflow already declares and dispatches it, the
# same call `gh workflow run <file> -f k=v ...` would make by hand. The
# actual procedure -- what evidence to gather first, which upstream run to
# select, when signing is required, how to verify a reputation-qualification
# before basing a publish on it, what "exact-source" and "no-rebuild
# Promotion" mean here -- is owned by the `run-reputation-and-release`,
# `sign-macos-artifacts` and `sign-windows-artifacts` skills (see this repo's
# .claude/skills/ and AGENTS.md's "Skills: look them up BEFORE acting").
# Read those first; this script is not a substitute for them, only for
# typing out `gh workflow run` by hand once you already know what to run.
#
# Usage:
#   scripts/ci-release.sh candidate  <source_sha> <upstream_run_id> [macos_signing_run_id]
#   scripts/ci-release.sh reputation <candidate_run_id> <source_sha>
#   scripts/ci-release.sh release    <candidate_run_id> <source_sha> <reputation_run_id> <version> <confirmation> [dry_run=true|false]
#
# Each subcommand prints the dispatched run's URL and watches it to
# completion (gh run watch --exit-status), same as scripts/ci-build.sh and
# scripts/ci-test.sh. It does not chain stages for you: read one stage's
# result (its run ID, and for Candidate its printed version/artifact
# identities) before dispatching the next, exactly as the skill's own
# references describe.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
# shellcheck source=lib/ci-dispatch.sh
source scripts/lib/ci-dispatch.sh

usage() {
  sed -n '2,26p' "$0" >&2
  exit 2
}

stage=${1:-}
shift || true

case "$stage" in
  candidate)
    source_sha=${1:-} upstream_run_id=${2:-} macos_signing_run_id=${3:-}
    [ -n "$source_sha" ] && [ -n "$upstream_run_id" ] || usage
    args=("source_sha=$source_sha" "upstream_run_id=$upstream_run_id")
    [ -n "$macos_signing_run_id" ] && args+=("macos_signing_run_id=$macos_signing_run_id")
    # The dispatch ref selects which copy of the *workflow file* runs, not
    # the source being released -- that's `source_sha`, passed as an input
    # above. `main` always has the current workflow file.
    minicon_ci_dispatch_and_wait candidate.yml main "${args[@]}"
    ;;
  reputation)
    candidate_run_id=${1:-} source_sha=${2:-}
    [ -n "$candidate_run_id" ] && [ -n "$source_sha" ] || usage
    minicon_ci_dispatch_and_wait reputation.yml main \
      "candidate_run_id=$candidate_run_id" "source_sha=$source_sha"
    ;;
  release)
    candidate_run_id=${1:-} source_sha=${2:-} reputation_run_id=${3:-} \
      version=${4:-} confirmation=${5:-} dry_run=${6:-true}
    [ -n "$candidate_run_id" ] && [ -n "$source_sha" ] && [ -n "$reputation_run_id" ] \
      && [ -n "$version" ] && [ -n "$confirmation" ] || usage
    minicon_ci_dispatch_and_wait release.yml main \
      "candidate_run_id=$candidate_run_id" "source_sha=$source_sha" \
      "reputation_run_id=$reputation_run_id" "version=$version" \
      "confirmation=$confirmation" "dry_run=$dry_run"
    ;;
  *)
    usage
    ;;
esac
