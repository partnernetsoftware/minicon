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
upload_jobs=${MINICON_OCI_UPLOAD_JOBS:-6}
evidence_probe_cell=${MINICON_EVIDENCE_PROBE_CELL:-none}
case "$package" in ghcr.io/*/*) ;; *) echo "package must be ghcr.io/OWNER/NAME" >&2; exit 2 ;; esac
case "$suite" in status|test|full) ;; *) echo "invalid suite: $suite" >&2; exit 2 ;; esac
case "$upload_jobs" in ''|*[!0-9]*) echo "MINICON_OCI_UPLOAD_JOBS must be an integer from 1 to 6" >&2; exit 2 ;; esac
[ "$upload_jobs" -ge 1 ] && [ "$upload_jobs" -le 6 ] || {
  echo "MINICON_OCI_UPLOAD_JOBS must be an integer from 1 to 6" >&2
  exit 2
}
case "$evidence_probe_cell" in
  none|lnx-x86_64|lnx-aarch64|win-x86_64|win-aarch64|osx-x86_64|osx-aarch64) ;;
  *) echo "invalid MINICON_EVIDENCE_PROBE_CELL: $evidence_probe_cell" >&2; exit 2 ;;
esac

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
started=$(date +%s)
for tool in gh jq oras python3; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

python3 scripts/package-six-grid-runtime.py
packaged=$(date +%s)
identity=$(jq -r '.source_tree_sha256' target-six/receipt.json)
manifest="target-six/cloud-runtime/minicon-six-grid-$identity-manifest.json"
source_sha=$(jq -r '.source_sha' "$manifest")
manifest_sha=$(shasum -a 256 "$manifest" | awk '{print $1}')
aggregator="scripts/aggregate-six-grid-runtime.py"
aggregator_sha=$(shasum -a 256 "$aggregator" | awk '{print $1}')
published_index="target-six/cloud-runtime/minicon-six-grid-$identity-published.json"
source_repo=${MINICON_SOURCE_GITHUB_REPO:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}
source_url="https://github.com/$source_repo"

jq -n --arg source_sha "$source_sha" --arg identity "$identity" \
  --arg package "$package" --arg manifest_sha "$manifest_sha" \
  --arg aggregator_sha "$aggregator_sha" \
  '{schema:2,source_sha:$source_sha,source_tree_sha256:$identity,package:$package,build_manifest_sha256:$manifest_sha,runtime_aggregator_sha256:$aggregator_sha,cells:{}}' \
  >"$published_index"

cells=(lnx-aarch64 lnx-x86_64 win-aarch64 win-x86_64 osx-aarch64 osx-x86_64)
scratch=$(mktemp -d "target-six/cloud-runtime/.publish-$identity.XXXXXX")
cleanup() {
  for cell in "${cells[@]}"; do rm -f "$scratch/$cell.digest"; done
  rmdir "$scratch" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

upload_cell() {
  cell=$1
  asset="target-six/cloud-runtime/minicon-six-grid-$identity-$cell.tar.gz"
  cell=${asset##*-"$identity"-}; cell=${cell%.tar.gz}
  output=$(oras push --no-tty --format json \
    --annotation "org.opencontainers.image.source=$source_url" \
    "$package:$identity-$cell" \
    "$asset:application/vnd.minicon.runtime-body.v1+tar+gzip")
  digest=$(printf '%s' "$output" | jq -r '.digest // .manifest.digest')
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "invalid OCI digest for $cell" >&2; exit 1; }
  printf '%s\n' "$digest" >"$scratch/$cell.digest"
}

upload_started=$(date +%s)
offset=0
while [ "$offset" -lt "${#cells[@]}" ]; do
  pids=()
  batch_cells=()
  limit=$((offset + upload_jobs))
  [ "$limit" -le "${#cells[@]}" ] || limit=${#cells[@]}
  while [ "$offset" -lt "$limit" ]; do
    cell=${cells[$offset]}
    upload_cell "$cell" &
    pids+=("$!")
    batch_cells+=("$cell")
    offset=$((offset + 1))
  done
  batch_failed=0
  for index in "${!pids[@]}"; do
    if ! wait "${pids[$index]}"; then
      echo "OCI layer upload failed for ${batch_cells[$index]}" >&2
      batch_failed=1
    fi
  done
  [ "$batch_failed" -eq 0 ] || exit 1
done

# Only the primary process mutates the published index, in canonical cell order.
# A missing or malformed worker result fails closed before the top-level push.
for cell in "${cells[@]}"; do
  digest=$(cat "$scratch/$cell.digest")
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "invalid recorded OCI digest for $cell" >&2; exit 1; }
  tmp="$published_index.tmp"
  jq --arg cell "$cell" --arg ref "$package@$digest" '.cells[$cell] = $ref' "$published_index" >"$tmp"
  mv "$tmp" "$published_index"
done
layers_uploaded=$(date +%s)

index_output=$(oras push --no-tty --format json \
  --annotation "org.opencontainers.image.source=$source_url" \
  "$package:$identity" \
  "$published_index:application/vnd.minicon.six-grid-index.v1+json" \
  "$manifest:application/vnd.minicon.build-manifest.v1+json" \
  "$aggregator:application/vnd.minicon.runtime-aggregator.v1+python")
index_digest=$(printf '%s' "$index_output" | jq -r '.digest // .manifest.digest')
[[ "$index_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "invalid OCI index digest" >&2; exit 1; }
bundle_ref="$package@$index_digest"
gh workflow run six-grid-runtime.yml --repo "$source_repo" \
  -f bundle_ref="$bundle_ref" -f source_sha="$source_sha" \
  -f source_tree_sha256="$identity" -f suite="$suite" \
  -f evidence_probe_cell="$evidence_probe_cell"
payload_bytes=$(jq '[.assets[].payload_bytes] | add' "$manifest")
archive_bytes=$(jq '[.assets[].archive_bytes] | add' "$manifest")
elapsed=$(( $(date +%s) - started ))
printf 'published=%s\ndispatched=%s/.github/workflows/six-grid-runtime.yml\npayload_bytes=%s\narchive_bytes=%s\ncompression_ratio=%s\nupload_jobs=%s\npackage_seconds=%s\nlayer_upload_seconds=%s\nelapsed_seconds=%s\n' \
  "$bundle_ref" "$source_repo" "$payload_bytes" "$archive_bytes" \
  "$(jq -n --argjson payload "$payload_bytes" --argjson archive "$archive_bytes" '$archive / $payload')" \
  "$upload_jobs" "$((packaged - started))" "$((layers_uploaded - upload_started))" "$elapsed"
