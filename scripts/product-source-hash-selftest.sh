#!/bin/bash
# Proves scripts/product-source-hash.sh does the one thing it exists for:
# a workflow/doc-only change must NOT move the hash, and a product-file
# change MUST move it. Built from a throwaway git repo, not this checkout.
set -euo pipefail

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/product-source-hash.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cd "$work"
git init -q
git config user.email test@example.invalid
git config user.name test

mkdir -p src .github/workflows prd
echo 'fn main() {}' >src/main.rs
echo 'name: x' >.github/workflows/ci.yml
echo '# notes' >prd/PRD_00_x.md
echo '# agents' >AGENTS.md
git add -A
git commit -q -m base
base_hash="$("$SCRIPT" HEAD)"

# Workflow-only change: hash must stay identical.
echo 'name: x changed' >.github/workflows/ci.yml
git add -A
git commit -q -m "workflow only"
workflow_hash="$("$SCRIPT" HEAD)"
if [[ "$workflow_hash" != "$base_hash" ]]; then
  echo "FAIL: a workflow-only change moved the product source hash" >&2
  exit 1
fi

# PRD-doc-only change: hash must also stay identical.
echo '# notes changed' >prd/PRD_00_x.md
git add -A
git commit -q -m "prd only"
prd_hash="$("$SCRIPT" HEAD)"
if [[ "$prd_hash" != "$base_hash" ]]; then
  echo "FAIL: a prd-doc-only change moved the product source hash" >&2
  exit 1
fi

# Product-file change: hash MUST move -- this is the guard's whole point.
echo 'fn main() { println!("x"); }' >src/main.rs
git add -A
git commit -q -m "product change"
product_hash="$("$SCRIPT" HEAD)"
if [[ "$product_hash" == "$base_hash" ]]; then
  echo "FAIL: a real product source change did not move the hash" >&2
  exit 1
fi

echo "product-source-hash-selftest: PASS"
