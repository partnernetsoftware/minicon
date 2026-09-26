#!/usr/bin/env bash
set -euo pipefail

script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/sign-windows-artifacts-self-test.XXXXXX")
cleanup() {
  if [[ -n "${test_root:-}" && -d "$test_root" && "$test_root" == *sign-windows-artifacts-self-test.* ]]; then
    rm -rf -- "$test_root"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL $1" >&2
  exit 1
}

expect_success() {
  local label=$1
  shift
  if ! "$@" >"$test_root/$label.out" 2>"$test_root/$label.err"; then
    cat "$test_root/$label.out" "$test_root/$label.err" >&2
    fail "$label expected success"
  fi
}

expect_failure_with() {
  local label=$1
  local needle=$2
  shift 2
  if "$@" >"$test_root/$label.out" 2>"$test_root/$label.err"; then
    fail "$label expected failure"
  fi
  if ! grep -Fq -- "$needle" "$test_root/$label.out" "$test_root/$label.err"; then
    cat "$test_root/$label.out" "$test_root/$label.err" >&2
    fail "$label did not report: $needle"
  fi
}

expect_success receipt \
  python3 "$script_root/audit-public-signing-receipt.py" --self-test

product="$test_root/product"
remote="$test_root/product.git"
mkdir -p "$product/.github/workflows" "$product/scripts"
git init -q -b main "$product"
git init -q --bare "$remote"
git -C "$product" config user.name "Signing Self Test"
git -C "$product" config user.email "signing-self-test@example.invalid"
printf '%s\n' '[package]' 'name = "signing-self-test"' 'version = "0.0.1"' >"$product/Cargo.toml"
printf '%s\n' '{"version":"0.0.1","signing":{"mode":"off"}}' >"$product/release-policy.json"
printf '%s\n' 'name: Candidate fixture' >"$product/.github/workflows/candidate.yml"
printf '%s\n' 'name: Signing fixture' >"$product/.github/workflows/company-signing.yml"
cp "$script_root/inspect-authenticode.ps1" "$product/scripts/inspect-authenticode.ps1"
cp "$script_root/inspect-authenticode.sh" "$product/scripts/inspect-authenticode.sh"
cp "$script_root/fetch-microsoft-trust-bundle.sh" "$product/scripts/fetch-microsoft-trust-bundle.sh"
git -C "$product" add -- .
git -C "$product" commit -q -m "fixture"
git -C "$product" remote add origin "$remote"
git -C "$product" push -q -u origin main

expect_success unpublished \
  "$script_root/check-product-signing-readiness.sh" "$product"

git -C "$product" tag v0.0.1
git -C "$product" push -q origin v0.0.1
expect_failure_with published-release "version_already_published=v0.0.1" \
  "$script_root/check-product-signing-readiness.sh" "$product"
expect_success published-qualification \
  "$script_root/check-product-signing-readiness.sh" "$product" --qualification

# A historical qualification has two identities: immutable payload source and
# current controller. The controller may evolve signing machinery, but cannot
# change the Candidate bytes or make the output release-eligible.
historical_sha=$(git -C "$product" rev-parse HEAD)
printf '%s\n' \
  'name: Historical signing fixture' \
  'source_class=immutable-version-tag' \
  'refs/tags/$tag^{}' \
  'release_eligible": False' \
  'candidate-part-windows-x86_64' \
  'candidate-part-windows-aarch64' \
  >"$product/.github/workflows/windows-signing-qualification.yml"
git -C "$product" add -- .github/workflows/windows-signing-qualification.yml
git -C "$product" commit -q -m "controller fixture"
git -C "$product" push -q origin main
historical_run=12345

printf '%s\n' '# dirty' >>"$product/Cargo.toml"
expect_failure_with dirty "worktree_dirty_count=1" \
  "$script_root/check-product-signing-readiness.sh" "$product" --qualification
git -C "$product" restore -- Cargo.toml

printf '%s\n' '# drift' >>"$product/scripts/inspect-authenticode.sh"
expect_failure_with inspector-drift "DRIFT scripts/inspect-authenticode.sh" \
  "$script_root/check-product-inspectors.sh" "$product"
git -C "$product" restore -- scripts/inspect-authenticode.sh

mock_bin="$test_root/bin"
mkdir -p "$mock_bin"
# The single-quoted expressions below belong to the generated mock, not this
# self-test process.
# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'if [[ "${1:-}" == api && "${2:-}" == "repos/example.invalid/product/actions/runs/12345" ]]; then' \
  '  printf "%s\n" '\''{"repository":{"full_name":"example.invalid/product"},"path":".github/workflows/candidate.yml","event":"workflow_dispatch","status":"completed","conclusion":"success","head_sha":"'"$historical_sha"'"}'\''' \
  '  exit 0' \
  'fi' \
  'if [[ "${1:-}" == api && "$*" == *"actions/runs/12345/artifacts?per_page=100"* ]]; then' \
  '  printf "%s\n" '\''{"artifacts":[{"name":"candidate-part-windows-x86_64","expired":false},{"name":"candidate-part-windows-aarch64","expired":false},{"name":"release-candidate-12345","expired":false}]}'\''' \
  '  exit 0' \
  'fi' \
  'endpoint=${2:-}' \
  'case "$endpoint" in' \
  '  */environments/release-signing) printf "%s\n" release-signing ;;' \
  '  */environments/release-signing/secrets) printf "%s\n" APPLE_NOTARY_KEY_ID,AZURE_CLIENT_ID,AZURE_SUBSCRIPTION_ID,AZURE_TENANT_ID ;;' \
  '  */environments/release-signing/variables)' \
  '    if [[ "${SIGNING_SELF_TEST_BAD_VARIABLES:-0}" == 1 ]]; then' \
  '      printf "%s\n" ARTIFACT_SIGNING_ACCOUNT' \
  '    else' \
  '      printf "%s\n" AGENTERM_APPLE_TEAM_ID,ARTIFACT_SIGNING_ACCOUNT,ARTIFACT_SIGNING_ENDPOINT,ARTIFACT_SIGNING_PROFILE' \
  '    fi' \
  '    ;;' \
  '  *) exit 2 ;;' \
  'esac' >"$mock_bin/gh"
chmod +x "$mock_bin/gh"

expect_success historical-qualification \
  env PATH="$mock_bin:$PATH" \
  "$script_root/check-historical-signing-qualification.sh" \
    "$product" example.invalid/product "$historical_sha" "$historical_run"

expect_success environment-names \
  env PATH="$mock_bin:$PATH" \
  "$script_root/check-github-signing-environment.sh" example.invalid/product
expect_failure_with environment-drift "variable is missing: ARTIFACT_SIGNING_ENDPOINT" \
  env PATH="$mock_bin:$PATH" SIGNING_SELF_TEST_BAD_VARIABLES=1 \
  "$script_root/check-github-signing-environment.sh" example.invalid/product

# shellcheck disable=SC2016
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'profile="/subscriptions/0/resourceGroups/fixture/providers/Microsoft.CodeSigning/codeSigningAccounts/fixture/certificateProfiles/public"' \
  'case "$*" in' \
  '  "account show --query id -o tsv") printf "%s\n" subscription-fixture ;;' \
  '  "trustedsigning list -o json") printf "%s\n" '\''[{"name":"fixture","location":"eastus","sku":{"name":"Basic"},"id":"/subscriptions/0/resourceGroups/fixture/providers/Microsoft.CodeSigning/codeSigningAccounts/fixture"}]'\'' ;;' \
  '  trustedsigning\ certificate-profile\ list*) printf "[{\"id\":\"%s\",\"status\":\"Active\"}]\n" "$profile" ;;' \
  '  "role assignment list --all -o json")' \
  '    principal_type=ServicePrincipal' \
  '    if [[ "${SIGNING_SELF_TEST_BAD_ROLE:-0}" == 1 ]]; then principal_type=User; fi' \
  '    printf "[{\"scope\":\"%s\",\"roleDefinitionName\":\"Artifact Signing Certificate Profile Signer\",\"principalType\":\"%s\",\"principalId\":\"principal-fixture\"}]\n" "$profile" "$principal_type"' \
  '    ;;' \
  '  "ad sp show --id principal-fixture --query appId -o tsv") printf "%s\n" app-fixture ;;' \
  '  "ad app federated-credential list --id app-fixture -o json") printf "%s\n" '\''[{"issuer":"https://token.actions.githubusercontent.com","subject":"repo:example@1/product@2:environment:release-signing"}]'\'' ;;' \
  '  *) exit 2 ;;' \
  'esac' >"$mock_bin/az"
chmod +x "$mock_bin/az"

expect_success azure-state \
  env PATH="$mock_bin:$PATH" \
  "$script_root/check-azure-signing-state.sh"
expect_failure_with azure-role-drift "non-service-principal or unexpected role" \
  env PATH="$mock_bin:$PATH" SIGNING_SELF_TEST_BAD_ROLE=1 \
  "$script_root/check-azure-signing-state.sh"

echo "PASS sign-windows-artifacts deterministic tool courts"
