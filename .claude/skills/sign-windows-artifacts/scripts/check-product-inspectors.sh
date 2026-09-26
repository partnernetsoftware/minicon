#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: check-product-inspectors.sh PRODUCT_REPOSITORY_ROOT" >&2
  exit 64
fi

product_root=$1
if [[ ! -d "$product_root" ]]; then
  echo "check-product-inspectors: repository root is not a directory" >&2
  exit 66
fi

script_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
failed=0
for name in inspect-authenticode.ps1 inspect-authenticode.sh fetch-microsoft-trust-bundle.sh; do
  product_copy="$product_root/scripts/$name"
  if [[ ! -f "$product_copy" ]]; then
    echo "MISSING scripts/$name" >&2
    failed=1
    continue
  fi
  if ! cmp -s -- "$script_root/$name" "$product_copy"; then
    echo "DRIFT scripts/$name (expected pns-authenticode-inspector/v3)" >&2
    failed=1
    continue
  fi
  echo "OK scripts/$name pns-authenticode-inspector/v3"
done

exit "$failed"
