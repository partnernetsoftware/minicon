#!/bin/bash
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
workflow="$repo_root/.github/workflows/six-grid-runtime.yml"

python3 -m py_compile "$repo_root/scripts/package-six-grid-runtime.py"
python3 -m py_compile "$repo_root/scripts/aggregate-six-grid-runtime.py"
python3 "$repo_root/scripts/aggregate-six-grid-runtime-selftest.py"
bash -n "$repo_root/scripts/publish-six-grid-runtime.sh"
bash "$repo_root/scripts/publish-six-grid-runtime-selftest.sh"

for forbidden in 'cargo ' 'rustup ' 'actions/checkout'; do
  if grep -F "$forbidden" "$workflow" >/dev/null; then
    echo "runtime workflow contains forbidden build operation: $forbidden" >&2
    exit 1
  fi
done
for runner in ubuntu-24.04 ubuntu-24.04-arm windows-2025 windows-11-arm macos-15-intel macos-15; do
  grep -F "runner: $runner" "$workflow" >/dev/null || {
    echo "missing native runner: $runner" >&2
    exit 1
  }
done
for architecture in X64 ARM64; do
  grep -F "expected_arch: $architecture" "$workflow" >/dev/null || {
    echo "missing runtime architecture assertion: $architecture" >&2
    exit 1
  }
done
grep -F "test \"\$RUNNER_ARCH\" = \"\$EXPECTED_RUNNER_ARCH\"" "$workflow" >/dev/null
grep -F "test \"\$RUNNER_OS\" = \"\$EXPECTED_RUNNER_OS\"" "$workflow" >/dev/null
grep -F 'packages: read' "$workflow" >/dev/null
grep -F '@sha256:' "$workflow" >/dev/null
grep -F 'timeout-minutes: 20' "$workflow" >/dev/null
grep -F 'run_attempt' "$workflow" >/dev/null
grep -F 'runtime-body.log' "$workflow" >/dev/null
grep -F 'reverified-pass' "$repo_root/scripts/aggregate-six-grid-runtime.py" >/dev/null
printf 'six-grid-cloud-selftest: PASS\n'
