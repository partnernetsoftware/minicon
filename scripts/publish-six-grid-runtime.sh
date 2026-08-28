#!/bin/bash
# Publish exact local cross-build bodies as immutable GHCR OCI artifacts, then
# dispatch the source repository's native six-grid runtime workflow by digest.

set -euo pipefail

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || {
  echo "usage: scripts/publish-six-grid-runtime.sh ghcr.io/OWNER/PACKAGE [status|test|full]" >&2
  exit 2
}
package=$1
suite=${2:-test}
case "$package" in ghcr.io/*/*) ;; *) echo "package must be ghcr.io/OWNER/NAME" >&2; exit 2 ;; esac
case "$suite" in status|test|full) ;; *) echo "invalid suite: $suite" >&2; exit 2 ;; esac

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
started=$(date +%s)
for tool in gh jq oras python3; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

python3 scripts/package-six-grid-runtime.py
identity=$(jq -r '.source_tree_sha256' target-six/receipt.json)
manifest="target-six/cloud-runtime/minicon-six-grid-$identity-manifest.json"
source_sha=$(jq -r '.source_sha' "$manifest")
manifest_sha=$(shasum -a 256 "$manifest" | awk '{print $1}')
published_index="target-six/cloud-runtime/minicon-six-grid-$identity-published.json"
source_repo=${MINICON_SOURCE_GITHUB_REPO:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}
source_url="https://github.com/$source_repo"

printf '{"schema":1,"source_sha":"%s","source_tree_sha256":"%s","package":"%s","build_manifest_sha256":"%s","cells":{}}\n' \
  "$source_sha" "$identity" "$package" "$manifest_sha" >"$published_index"
for asset in target-six/cloud-runtime/minicon-six-grid-"$identity"-{lnx-aarch64,lnx-x86_64,win-aarch64,win-x86_64,osx-aarch64,osx-x86_64}.tar.gz; do
  cell=${asset##*-"$identity"-}; cell=${cell%.tar.gz}
  output=$(oras push --no-tty --format json \
    --annotation "org.opencontainers.image.source=$source_url" \
    "$package:$identity-$cell" \
    "$asset:application/vnd.minicon.runtime-body.v1+tar+gzip")
  digest=$(printf '%s' "$output" | jq -r '.digest // .manifest.digest')
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "invalid OCI digest for $cell" >&2; exit 1; }
  tmp="$published_index.tmp"
  jq --arg cell "$cell" --arg ref "$package@$digest" '.cells[$cell] = $ref' "$published_index" >"$tmp"
  mv "$tmp" "$published_index"
done

index_output=$(oras push --no-tty --format json \
  --annotation "org.opencontainers.image.source=$source_url" \
  "$package:$identity" \
  "$published_index:application/vnd.minicon.six-grid-index.v1+json" \
  "$manifest:application/vnd.minicon.build-manifest.v1+json")
index_digest=$(printf '%s' "$index_output" | jq -r '.digest // .manifest.digest')
[[ "$index_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "invalid OCI index digest" >&2; exit 1; }
bundle_ref="$package@$index_digest"
gh workflow run six-grid-runtime.yml --repo "$source_repo" \
  -f bundle_ref="$bundle_ref" -f source_sha="$source_sha" \
  -f source_tree_sha256="$identity" -f suite="$suite"
bytes=$(jq '[.assets[].bytes] | add' "$manifest")
elapsed=$(( $(date +%s) - started ))
printf 'published=%s\ndispatched=%s/.github/workflows/six-grid-runtime.yml\nbytes=%s\nelapsed_seconds=%s\n' \
  "$bundle_ref" "$source_repo" "$bytes" "$elapsed"
