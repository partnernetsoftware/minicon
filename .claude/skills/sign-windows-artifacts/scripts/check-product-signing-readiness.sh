#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: check-product-signing-readiness.sh PRODUCT_REPOSITORY_ROOT [--qualification]" >&2
  exit 64
fi
purpose=release
if [[ $# -eq 2 ]]; then
  if [[ "$2" != "--qualification" ]]; then
    echo "usage: check-product-signing-readiness.sh PRODUCT_REPOSITORY_ROOT [--qualification]" >&2
    exit 64
  fi
  purpose=qualification
fi
for command_name in git jq sed; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "check-product-signing-readiness: required command is unavailable: $command_name" >&2
    exit 69
  fi
done

product_root=$1
if ! git -C "$product_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "check-product-signing-readiness: target is not a Git repository" >&2
  exit 66
fi

script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
policy="$product_root/release-policy.json"
ready=1

pass() { echo "READY_CHECK $1"; }
fail() { echo "NOT_READY $1" >&2; ready=0; }

if [[ ! -f "$policy" ]]; then
  fail "release_policy_missing"
  version=""
  signing_mode=""
else
  version=$(jq -er '.version | strings | select(test("^[0-9]+\\.[0-9]+\\.[0-9]+$"))' "$policy" 2>/dev/null || true)
  if [[ -z "$version" ]]; then
    fail "release_policy_version_invalid"
  else
    pass "release_policy_version=$version"
  fi
  signing_mode=$(jq -er 'if .signing.windows then .signing.windows elif .signing.mode then .signing.mode else empty end' "$policy" 2>/dev/null || true)
  case "$signing_mode" in
    off|required) pass "signing_mode=$signing_mode" ;;
    *) fail "signing_mode_invalid" ;;
  esac
fi

cargo_version=""
if [[ -f "$product_root/Cargo.toml" ]]; then
  cargo_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$product_root/Cargo.toml" | head -n 1)
fi
if [[ -z "$version" || "$cargo_version" != "$version" ]]; then
  fail "cargo_and_release_policy_version_mismatch"
else
  pass "cargo_version=$cargo_version"
fi

dirty_count=$(git -C "$product_root" status --porcelain=v1 --untracked-files=all | wc -l | tr -d ' ')
if [[ "$dirty_count" == 0 ]]; then
  pass "worktree_clean"
else
  fail "worktree_dirty_count=$dirty_count"
fi

head_sha=$(git -C "$product_root" rev-parse HEAD)
remote_main=$(git -C "$product_root" ls-remote origin refs/heads/main 2>/dev/null | awk 'NR == 1 { print $1 }')
if [[ "$head_sha" =~ ^[0-9a-f]{40}$ && "$remote_main" == "$head_sha" ]]; then
  pass "exact_main_sha=$head_sha"
else
  fail "head_is_not_exact_remote_main"
fi

if [[ -n "$version" ]]; then
  set +e
  git -C "$product_root" ls-remote --exit-code --tags origin "refs/tags/v$version" >/dev/null 2>&1
  tag_status=$?
  set -e
  case "$tag_status" in
    0)
      if [[ "$purpose" == qualification ]]; then
        pass "published_version_allowed_for_nonpromotable_qualification=v$version"
      else
        fail "version_already_published=v$version"
      fi
      ;;
    2) pass "version_unpublished=v$version" ;;
    *) fail "remote_tag_state_unavailable" ;;
  esac
fi

if [[ ! -f "$product_root/.github/workflows/candidate.yml" ]]; then
  fail "candidate_workflow_missing"
else
  pass "candidate_workflow_present"
fi
if [[ -f "$product_root/.github/workflows/company-signing.yml" ||
      -f "$product_root/.github/workflows/windows-signing-qualification.yml" ]]; then
  pass "signing_workflow_present"
else
  fail "signing_workflow_missing"
fi

if "$script_root/check-product-inspectors.sh" "$product_root"; then
  pass "inspector_contract=pns-authenticode-inspector/v3"
else
  fail "inspector_contract_drift"
fi

if [[ "$ready" == 1 ]]; then
  echo "READY product source can enter the checked-in $purpose signing policy flow"
  exit 0
fi
echo "NOT_READY product source must not enter a signing workflow" >&2
exit 1
