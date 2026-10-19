#!/bin/bash
# Bounded local build/test entry. Direct Cargo remains available, but this
# wrapper is the documented path because it reclaims stale local build state.

set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"
mode=${1:-release}
shift || true

# The cleanup helper is Python, and the interpreter's name differs by host:
# Linux/macOS ship `python3`, a Windows Git-for-Windows shell usually has only
# `python` (or the `py` launcher). Pick whichever exists and say so plainly when
# none does, instead of dying mid-build with "python3: command not found".
python_bin=""
for candidate in python3 python py; do
  if command -v "$candidate" >/dev/null 2>&1; then
    python_bin="$candidate"
    break
  fi
done
if [ -z "$python_bin" ]; then
  echo "scripts/build.sh: no python interpreter found (tried python3, python, py)" >&2
  exit 127
fi

"$python_bin" scripts/cleanup-build-state.py --apply --scope routine
mkdir -p target
marker=target/.minicon-build-active
printf '%s\n' "$$" >"$marker"
cleanup() { rm -f "$marker"; }
trap cleanup EXIT HUP INT TERM

case "$mode" in
  release) cargo build --locked --release "$@" ;;
  dev) cargo build --locked "$@" ;;
  check) cargo check --locked --workspace --all-targets "$@" ;;
  # Deny the lints that can silently disable a test. An unused function in a
  # test module compiles and runs nothing, and an unused `Result` hides a
  # failing setup, so the test gate treats both as errors. Passed per
  # invocation rather than through `RUSTFLAGS` so the build cache stays valid.
  #
  # The vendored vt100 fork is a `[patch.crates-io]` path, deliberately not a
  # workspace member, so `--workspace` never reaches its damage oracle
  # (`assert_damage_covers`). The gate therefore also runs the fork's own
  # suite from its own manifest and lock, scoped to `--lib`: the doctest step
  # needs a rustc-spawning environment the gate does not require, and the
  # oracle under guard lives in the library. plan-v0.2.10 {GATE-FORK}.
  test) cargo test --locked --workspace \
    --config 'build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]' \
    "$@"
    # Workspace feature names do not belong to the separately owned fork.
    # Preserve selectors and libtest arguments, including anything after `--`.
    fork_args=()
    skip_feature_value=false
    libtest_args=false
    for argument in "$@"; do
      if "$libtest_args"; then
        fork_args+=("$argument")
      elif "$skip_feature_value"; then
        skip_feature_value=false
      else
        case "$argument" in
          --) libtest_args=true; fork_args+=("$argument") ;;
          --features|-F) skip_feature_value=true ;;
          --features=*|-F?*) ;;
          *) fork_args+=("$argument") ;;
        esac
      fi
    done
    fork_lints='build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]'
    cargo test --locked --manifest-path third_party/vt100/Cargo.toml --config "$fork_lints" --lib "${fork_args[@]}"
    # The shared grammar boundary oracle belongs to the owned VTE lexer.
    cargo test --locked --manifest-path third_party/vte/Cargo.toml --config "$fork_lints" --lib "${fork_args[@]}" ;;
  *) echo "usage: scripts/build.sh [release|dev|check|test] [cargo arguments...]" >&2; exit 2 ;;
esac
