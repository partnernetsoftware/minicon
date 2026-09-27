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
#   scripts/ci-release.sh signing       <source_sha> <minicon_com_run_id> [--release|--qualification-only]
#   scripts/ci-release.sh macos-signing <source_sha> <minicon_com_run_id> [--release|--qualification-only]
#   scripts/ci-release.sh candidate     <source_sha> <upstream_run_id> [macos_signing_run_id]
#   scripts/ci-release.sh reputation    <candidate_run_id> <source_sha>
#   scripts/ci-release.sh release       <candidate_run_id> <source_sha> <reputation_run_id> <version> <confirmation> [dry_run=true|false]
#
# Each subcommand prints the dispatched run's URL and watches it to
# completion (gh run watch --exit-status), same as scripts/ci-build.sh and
# scripts/ci-test.sh. It does not chain stages for you: read one stage's
# result (its run ID, and for Candidate its printed version/artifact
# identities) before dispatching the next, exactly as the skill's own
# references describe.
#
# `--qualification-only` (the default for signing/macos-signing) matches
# each workflow's own `qualification_only: true` default -- exact-byte
# execution for provider qualification, never Candidate-eligible.
# `--release` passes `qualification_only=false`, which each workflow's own
# preflight only accepts for an unpublished version (see release-policy.json
# and company-signing.yml/macos-signing.yml's preflight jobs).
#
# The dispatch ref for every subcommand -- which copy of the *workflow file*
# runs, never the source being released (that is always source_sha, passed
# as an input) -- defaults to `main` and can be overridden with the
# MINICON_CI_REF environment variable, e.g. to pin a `candidate-src-<v>`
# branch once `main` has moved past the Candidate SHA.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
# shellcheck source=lib/ci-dispatch.sh
source scripts/lib/ci-dispatch.sh

usage() {
  sed -n '2,44p' "$0" >&2
  exit 2
}

ref=${MINICON_CI_REF:-main}

# qualification_only_flag <arg...> -- scans the remaining positional args for
# --release/--qualification-only, echoes "true"/"false", and errors on both
# or neither being an unrecognized non-empty extra arg.
qualification_only_flag() {
  local value=true arg
  for arg in "$@"; do
    case "$arg" in
      --release) value=false ;;
      --qualification-only) value=true ;;
      "") ;;
      *)
        echo "ci-release.sh: unrecognized flag: $arg" >&2
        usage
        ;;
    esac
  done
  echo "$value"
}

stage=${1:-}
shift || true

case "$stage" in
  signing)
    source_sha=${1:-} minicon_com_run_id=${2:-}
    [ -n "$source_sha" ] && [ -n "$minicon_com_run_id" ] || usage
    qualification_only=$(qualification_only_flag "${3:-}")
    minicon_ci_dispatch_and_wait company-signing.yml "$ref" \
      "source_sha=$source_sha" "minicon_com_run_id=$minicon_com_run_id" \
      "qualification_only=$qualification_only"
    ;;
  macos-signing)
    source_sha=${1:-} minicon_com_run_id=${2:-}
    [ -n "$source_sha" ] && [ -n "$minicon_com_run_id" ] || usage
    qualification_only=$(qualification_only_flag "${3:-}")
    minicon_ci_dispatch_and_wait macos-signing.yml "$ref" \
      "source_sha=$source_sha" "minicon_com_run_id=$minicon_com_run_id" \
      "qualification_only=$qualification_only"
    ;;
  candidate)
    source_sha=${1:-} upstream_run_id=${2:-} macos_signing_run_id=${3:-}
    [ -n "$source_sha" ] && [ -n "$upstream_run_id" ] || usage
    args=("source_sha=$source_sha" "upstream_run_id=$upstream_run_id")
    [ -n "$macos_signing_run_id" ] && args+=("macos_signing_run_id=$macos_signing_run_id")
    minicon_ci_dispatch_and_wait candidate.yml "$ref" "${args[@]}"
    ;;
  reputation)
    candidate_run_id=${1:-} source_sha=${2:-}
    [ -n "$candidate_run_id" ] && [ -n "$source_sha" ] || usage
    minicon_ci_dispatch_and_wait reputation.yml "$ref" \
      "candidate_run_id=$candidate_run_id" "source_sha=$source_sha"
    ;;
  release)
    candidate_run_id=${1:-} source_sha=${2:-} reputation_run_id=${3:-} \
      version=${4:-} confirmation=${5:-} dry_run=${6:-true}
    [ -n "$candidate_run_id" ] && [ -n "$source_sha" ] && [ -n "$reputation_run_id" ] \
      && [ -n "$version" ] && [ -n "$confirmation" ] || usage
    minicon_ci_dispatch_and_wait release.yml "$ref" \
      "candidate_run_id=$candidate_run_id" "source_sha=$source_sha" \
      "reputation_run_id=$reputation_run_id" "version=$version" \
      "confirmation=$confirmation" "dry_run=$dry_run"
    ;;
  *)
    usage
    ;;
esac
