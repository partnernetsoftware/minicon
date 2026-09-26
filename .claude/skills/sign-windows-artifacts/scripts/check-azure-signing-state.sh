#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 0 ]]; then
  echo "usage: check-azure-signing-state.sh" >&2
  exit 64
fi
for command_name in az jq; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "check-azure-signing-state: required command is unavailable: $command_name" >&2
    exit 69
  fi
done
if ! az account show --query id -o tsv >/dev/null 2>&1; then
  echo "NOT_READY Azure CLI session is not authenticated" >&2
  exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/check-azure-signing-state.XXXXXX")
cleanup() {
  rm -rf -- "$scratch"
}
trap cleanup EXIT
fail() {
  echo "NOT_READY $1" >&2
  exit 1
}

if ! az trustedsigning list -o json >"$scratch/accounts.json" 2>"$scratch/az.err"; then
  fail "Artifact Signing accounts are unreadable"
fi
account_count=$(jq 'length' "$scratch/accounts.json")
[[ "$account_count" == 1 ]] || fail "expected exactly one company Artifact Signing account"
if ! jq -e 'all(.[]; .location == "eastus" and .sku.name == "Basic")' \
  "$scratch/accounts.json" >/dev/null; then
  fail "Artifact Signing account location or SKU drift"
fi

: >"$scratch/profile-scopes.txt"
profile_count=0
while IFS=$'\t' read -r account_name resource_group; do
  if ! az trustedsigning certificate-profile list \
    --account-name "$account_name" --resource-group "$resource_group" \
    -o json >"$scratch/profiles-$profile_count.json" 2>"$scratch/az.err"; then
    fail "certificate profiles are unreadable"
  fi
  current_count=$(jq 'length' "$scratch/profiles-$profile_count.json")
  if ! jq -e 'all(.[]; (.status // .provisioningState) == "Active")' \
    "$scratch/profiles-$profile_count.json" >/dev/null; then
    fail "certificate profile is not Active"
  fi
  jq -r '.[].id' "$scratch/profiles-$profile_count.json" >>"$scratch/profile-scopes.txt"
  profile_count=$((profile_count + current_count))
done < <(jq -r '.[] | [.name, (.id | split("/")[4])] | @tsv' "$scratch/accounts.json")
[[ "$profile_count" == 1 ]] || fail "expected exactly one company Public Trust profile"

if ! az role assignment list --all -o json >"$scratch/roles.json" 2>"$scratch/az.err"; then
  fail "Azure role assignments are unreadable"
fi
jq -Rn '[inputs]' <"$scratch/profile-scopes.txt" >"$scratch/profile-scopes.json"
jq --slurpfile scopes "$scratch/profile-scopes.json" \
  '[.[] | select(.scope as $scope | $scopes[0] | index($scope))]' \
  "$scratch/roles.json" >"$scratch/profile-roles.json"

signer_count=$(jq 'length' "$scratch/profile-roles.json")
[[ "$signer_count" -gt 0 ]] || fail "certificate profile has no signer assignment"
if ! jq -e 'all(.[];
    .roleDefinitionName == "Artifact Signing Certificate Profile Signer" and
    .principalType == "ServicePrincipal")' "$scratch/profile-roles.json" >/dev/null; then
  fail "profile scope contains a non-service-principal or unexpected role"
fi

valid_subjects=0
identity_index=0
while IFS= read -r principal_id; do
  identity_index=$((identity_index + 1))
  if ! app_id=$(az ad sp show --id "$principal_id" --query appId -o tsv 2>/dev/null); then
    fail "signer service principal is unreadable"
  fi
  if ! az ad app federated-credential list --id "$app_id" -o json \
    >"$scratch/federation-$identity_index.json" 2>"$scratch/az.err"; then
    fail "signer federation is unreadable"
  fi
  credential_count=$(jq 'length' "$scratch/federation-$identity_index.json")
  matching_count=$(jq '[.[] | select(
      .issuer == "https://token.actions.githubusercontent.com" and
      (.subject | test("^repo:[A-Za-z0-9_.-]+@[0-9]+/[A-Za-z0-9_.-]+@[0-9]+:environment:release-signing$"))
    )] | length' "$scratch/federation-$identity_index.json")
  if [[ "$credential_count" != 1 || "$matching_count" != 1 ]]; then
    fail "signer must have exactly one immutable GitHub release-signing subject"
  fi
  valid_subjects=$((valid_subjects + matching_count))
done < <(jq -r '.[].principalId' "$scratch/profile-roles.json")

echo "READY Azure Artifact Signing control plane"
echo "READY_CHECK account_count=$account_count"
echo "READY_CHECK location=eastus"
echo "READY_CHECK sku=Basic"
echo "READY_CHECK active_profile_count=$profile_count"
echo "READY_CHECK profile_signer_service_principals=$signer_count"
echo "READY_CHECK immutable_github_environment_subjects=$valid_subjects"
echo "NOTE protected account, profile, tenant, subscription, application and principal identifiers were not printed"
