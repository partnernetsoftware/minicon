#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
  echo "usage: check-github-signing-environment.sh OWNER/REPOSITORY" >&2
  exit 64
fi
if ! command -v gh >/dev/null 2>&1; then
  echo "check-github-signing-environment: gh is required" >&2
  exit 69
fi

repository=$1
environment_name=$(gh api "repos/$repository/environments/release-signing" --jq '.name')
if [[ "$environment_name" != "release-signing" ]]; then
  echo "NOT_READY release-signing environment is missing" >&2
  exit 1
fi

secret_names=$(gh api "repos/$repository/environments/release-signing/secrets" \
  --jq '[.secrets[].name] | sort | join(",")')
variable_names=$(gh api "repos/$repository/environments/release-signing/variables" \
  --jq '[.variables[].name] | sort | join(",")')
required_secrets=(AZURE_CLIENT_ID AZURE_SUBSCRIPTION_ID AZURE_TENANT_ID)
required_variables=(ARTIFACT_SIGNING_ACCOUNT ARTIFACT_SIGNING_ENDPOINT ARTIFACT_SIGNING_PROFILE)

for name in "${required_secrets[@]}"; do
  if [[ ",$secret_names," != *",$name,"* ]]; then
    echo "NOT_READY release-signing secret is missing: $name" >&2
    exit 1
  fi
done
for name in "${required_variables[@]}"; do
  if [[ ",$variable_names," != *",$name,"* ]]; then
    echo "NOT_READY release-signing variable is missing: $name" >&2
    exit 1
  fi
done

echo "READY release-signing environment"
echo "READY_CHECK secret_names=$secret_names"
echo "READY_CHECK variable_names=$variable_names"
echo "NOTE values, OIDC subject, and Azure RBAC were not read or validated"
