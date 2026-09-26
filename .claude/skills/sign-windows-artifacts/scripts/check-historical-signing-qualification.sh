#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: check-historical-signing-qualification.sh PRODUCT_REPOSITORY_ROOT OWNER/REPOSITORY SOURCE_SHA CANDIDATE_RUN_ID" >&2
  exit 64
fi

for command_name in git gh jq sed; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "historical-signing-readiness: required command is unavailable: $command_name" >&2
    exit 69
  fi
done

product_root=$1
repository=$2
source_sha=$3
candidate_run_id=$4
script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "NOT_READY repository_invalid" >&2
  exit 1
}
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || {
  echo "NOT_READY source_sha_invalid" >&2
  exit 1
}
[[ "$candidate_run_id" =~ ^[1-9][0-9]*$ ]] || {
  echo "NOT_READY candidate_run_id_invalid" >&2
  exit 1
}
git -C "$product_root" rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "NOT_READY product_is_not_git_repository" >&2
  exit 1
}
git -C "$product_root" cat-file -e "$source_sha^{commit}" 2>/dev/null || {
  echo "NOT_READY source_commit_unavailable" >&2
  exit 1
}

remote_main=$(git -C "$product_root" ls-remote origin refs/heads/main | awk 'NR == 1 {print $1}')
[[ "$remote_main" =~ ^[0-9a-f]{40}$ ]] || {
  echo "NOT_READY remote_main_unavailable" >&2
  exit 1
}
[[ "$source_sha" != "$remote_main" ]] || {
  echo "NOT_READY source_is_current_main_use_standard_readiness" >&2
  exit 1
}

policy=$(git -C "$product_root" show "$source_sha:release-policy.json")
version=$(jq -er '.version | strings | select(test("^[0-9]+\\.[0-9]+\\.[0-9]+$"))' <<<"$policy")
[[ "$(jq -r 'if .signing.windows then .signing.windows elif .signing.mode then .signing.mode else empty end' <<<"$policy")" == off ]] || {
  echo "NOT_READY historical_source_signing_policy_is_not_off" >&2
  exit 1
}
cargo_version=$(
  git -C "$product_root" show "$source_sha:Cargo.toml" |
    sed -n 's/^version = "\([^"]*\)"/\1/p' | head -n1
)
[[ "$cargo_version" == "$version" ]] || {
  echo "NOT_READY historical_version_mismatch" >&2
  exit 1
}

tag="v$version"
tag_rows=$(git -C "$product_root" ls-remote origin "refs/tags/$tag" "refs/tags/$tag^{}")
direct_tag=$(awk -v ref="refs/tags/$tag" '$2 == ref {print $1}' <<<"$tag_rows")
peeled_tag=$(awk -v ref="refs/tags/$tag^{}" '$2 == ref {print $1}' <<<"$tag_rows")
resolved_tag=${peeled_tag:-$direct_tag}
[[ "$resolved_tag" == "$source_sha" ]] || {
  echo "NOT_READY immutable_version_tag_mismatch" >&2
  exit 1
}

git -C "$product_root" cat-file -e "$remote_main^{commit}" 2>/dev/null ||
  git -C "$product_root" fetch --no-tags origin "$remote_main" >/dev/null
workflow=$(git -C "$product_root" show "$remote_main:.github/workflows/windows-signing-qualification.yml")
for contract in \
  'source_class=immutable-version-tag' \
  'refs/tags/$tag^{}' \
  'release_eligible": False' \
  'candidate-part-windows-x86_64' \
  'candidate-part-windows-aarch64'
do
  grep -Fq -- "$contract" <<<"$workflow" || {
    echo "NOT_READY controller_workflow_contract_missing" >&2
    exit 1
  }
done

run=$(gh api "repos/$repository/actions/runs/$candidate_run_id")
[[ "$(jq -r .repository.full_name <<<"$run")" == "$repository" ]]
[[ "$(jq -r .path <<<"$run")" == .github/workflows/candidate.yml ]]
[[ "$(jq -r .event <<<"$run")" == workflow_dispatch ]]
[[ "$(jq -r .status <<<"$run")" == completed ]]
[[ "$(jq -r .conclusion <<<"$run")" == success ]]
[[ "$(jq -r .head_sha <<<"$run")" == "$source_sha" ]]

artifacts=$(gh api --paginate "repos/$repository/actions/runs/$candidate_run_id/artifacts?per_page=100")
for artifact_name in \
  candidate-part-windows-x86_64 \
  candidate-part-windows-aarch64 \
  "release-candidate-$candidate_run_id"
do
  count=$(jq -s --arg name "$artifact_name" \
    '[.[].artifacts[] | select(.name == $name and (.expired | not))] | length' \
    <<<"$artifacts")
  [[ "$count" == 1 ]] || {
    echo "NOT_READY candidate_artifact_unavailable=$artifact_name" >&2
    exit 1
  }
done

"$script_root/check-product-inspectors.sh" "$product_root" >/dev/null
echo "READY historical non-promotable signing qualification"
echo "READY_CHECK source_class=immutable-version-tag"
echo "READY_CHECK version_tag=$tag"
echo "READY_CHECK source_sha=$source_sha"
echo "READY_CHECK candidate_run_id=$candidate_run_id"
echo "READY_CHECK controller_sha=$remote_main"
