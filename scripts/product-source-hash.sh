#!/bin/bash
# Prints a hash of the tracked product source tree at a given ref, excluding
# paths whose content cannot change what a release actually builds/ships:
# workflow orchestration, product-boundary/planning docs, and top-level
# human-facing docs. release-policy.json and Cargo.* stay IN scope -- they
# select what gets built and signed, so a change there is a real release
# change, not an infra-only one.
#
# Purpose: the exact-source Candidate invariant used to compare raw
# `git rev-parse origin/main` against the pinned source_sha, so a CI-only fix
# (a workflow regex, a doc correction) landing on main mid-chain invalidated
# every in-flight Candidate/Defender/Reputation/Release run for that version,
# forcing a version bump for a bug that never touched a shipped byte. This
# hash lets those assertions compare "did the product actually change" instead
# of "did main move at all", so the same version number can absorb an
# infra-only fix landing on main mid-release.
#
# Usage: scripts/product-source-hash.sh <git-ref>
set -euo pipefail

# Deliberately does NOT cd to this script's own repo: a caller (CI step, or
# this script's own selftest) may be operating on a different working tree's
# git history, and `git ls-tree`/`git rev-parse` below resolve against
# whatever repo the current working directory belongs to.
ref="${1:?usage: scripts/product-source-hash.sh <git-ref>}"

EXCLUDE_RE='^(\.github/workflows/|prd/|plan/|AGENTS\.md$|PRD\.md$|README\.md$)'

git ls-tree -r --name-only "$ref" \
  | grep -vE "$EXCLUDE_RE" \
  | LC_ALL=C sort \
  | while IFS= read -r path; do
      printf '%s %s\n' "$(git rev-parse "$ref:$path")" "$path"
    done \
  | sha256sum \
  | cut -d' ' -f1
