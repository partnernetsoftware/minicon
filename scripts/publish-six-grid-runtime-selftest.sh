#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/minicon-publish-selftest.XXXXXX")
cleanup() { rm -rf "$fixture"; }
trap cleanup EXIT

mkdir -p "$fixture/scripts" "$fixture/target-six/cloud-runtime" "$fixture/bin"
cp "$repo_root/scripts/publish-six-grid-runtime.sh" "$fixture/scripts/"
identity=1111111111111111111111111111111111111111111111111111111111111111
source_sha=2222222222222222222222222222222222222222
printf '{"source_tree_sha256":"%s"}\n' "$identity" >"$fixture/target-six/receipt.json"

cells=(lnx-aarch64 lnx-x86_64 win-aarch64 win-x86_64 osx-aarch64 osx-x86_64)
assets='[]'
for cell in "${cells[@]}"; do
  asset="minicon-six-grid-$identity-$cell.tar.gz"
  : >"$fixture/target-six/cloud-runtime/$asset"
  assets=$(jq --arg cell "$cell" --arg asset "$asset" \
    '. + [{cell:$cell,asset:$asset,payload_bytes:100,archive_bytes:40,bytes:40,sha256:"unused"}]' <<<"$assets")
done
jq -n --arg source_sha "$source_sha" --arg identity "$identity" --argjson assets "$assets" \
  '{schema:1,source_sha:$source_sha,source_tree_sha256:$identity,assets:$assets}' \
  >"$fixture/target-six/cloud-runtime/minicon-six-grid-$identity-manifest.json"

cat >"$fixture/bin/python3" <<'EOF'
#!/bin/bash
exit 0
EOF
cat >"$fixture/bin/oras" <<'EOF'
#!/bin/bash
set -euo pipefail
ref=
for arg in "$@"; do case "$arg" in ghcr.io/*:*) ref=$arg; break ;; esac; done
[ -n "$ref" ]
printf 'start:%s\n' "$ref" >>"$ORAS_LOG"
if [ -n "${FAIL_CELL:-}" ]; then
  case "$ref" in *-"$FAIL_CELL") exit 9 ;; esac
fi
case "$ref" in *-lnx-*|*-win-*|*-osx-*) sleep 1 ;; esac
printf 'end:%s\n' "$ref" >>"$ORAS_LOG"
printf '{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}\n'
EOF
cat >"$fixture/bin/gh" <<'EOF'
#!/bin/bash
set -euo pipefail
if [ "${1:-}" = repo ]; then
  printf 'example/minicon\n'
else
  printf '%s\n' "$*" >>"$GH_LOG"
fi
EOF
chmod +x "$fixture/bin/python3" "$fixture/bin/oras" "$fixture/bin/gh"

export PATH="$fixture/bin:$PATH" ORAS_LOG="$fixture/oras.log" GH_LOG="$fixture/gh.log"
output=$(cd "$fixture" && MINICON_OCI_UPLOAD_JOBS=6 scripts/publish-six-grid-runtime.sh ghcr.io/example/minicon-six-grid test)

[ "$(sed -n '1,6p' "$ORAS_LOG" | grep -c '^start:')" -eq 6 ]
[ "$(grep -c '^start:' "$ORAS_LOG")" -eq 7 ]
[ "$(grep -c '^end:' "$ORAS_LOG")" -eq 7 ]
grep -F 'upload_jobs=6' <<<"$output" >/dev/null
grep -F 'payload_bytes=600' <<<"$output" >/dev/null
grep -F 'archive_bytes=240' <<<"$output" >/dev/null
grep -F 'workflow run six-grid-runtime.yml' "$GH_LOG" >/dev/null
published="$fixture/target-six/cloud-runtime/minicon-six-grid-$identity-published.json"
[ "$(jq '.cells | length' "$published")" -eq 6 ]
for cell in "${cells[@]}"; do
  [ "$(jq -r --arg cell "$cell" '.cells[$cell]' "$published")" = \
    'ghcr.io/example/minicon-six-grid@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ]
done

: >"$ORAS_LOG"
: >"$GH_LOG"
if (cd "$fixture" && FAIL_CELL=win-aarch64 MINICON_OCI_UPLOAD_JOBS=6 \
  scripts/publish-six-grid-runtime.sh ghcr.io/example/minicon-six-grid test >/dev/null 2>&1); then
  echo 'publisher sealed an index after a layer failure' >&2
  exit 1
fi
[ "$(grep -c '^start:' "$ORAS_LOG")" -eq 6 ]
if grep -Fx "start:ghcr.io/example/minicon-six-grid:$identity" "$ORAS_LOG" >/dev/null; then
  echo 'publisher pushed the top-level index after a layer failure' >&2
  exit 1
fi
[ ! -s "$GH_LOG" ]

if (cd "$fixture" && MINICON_OCI_UPLOAD_JOBS=0 scripts/publish-six-grid-runtime.sh ghcr.io/example/minicon-six-grid test >/dev/null 2>&1); then
  echo 'publisher accepted invalid upload concurrency' >&2
  exit 1
fi

printf 'publish-six-grid-runtime-selftest: PASS\n'
