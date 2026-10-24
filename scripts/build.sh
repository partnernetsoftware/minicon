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

test_lints='build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]'

assemble_macos_dev_app() {
  [ "$(uname -s)" = Darwin ] || return 0
  dev_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
  dev_app="target/debug/MiniCon $dev_version Dev.app"
  dev_contents="$dev_app/Contents"
  # This is an ignored local test wrapper, never a release/signing artifact.
  # Keep its identity distinct so Computer Use does not select an installed
  # MiniCon while qualifying the binary that was just built.
  rm -rf "$dev_app"
  mkdir -p "$dev_contents/MacOS" "$dev_contents/Resources"
  cp target/debug/minicon "$dev_contents/MacOS/minicon"
  cp assets/minicon.icns "$dev_contents/Resources/minicon.icns"
  "$python_bin" - "$dev_contents/Info.plist" "$dev_version" <<'PY'
import plistlib
import sys

version = sys.argv[2]
payload = {
    "CFBundleName": f"MiniCon {version} Dev",
    "CFBundleDisplayName": f"MiniCon {version} Dev",
    "CFBundleIdentifier": f"com.partnernetsoftware.minicon.dev-{version.replace('.', '-')}",
    "CFBundleExecutable": "minicon",
    "CFBundleIconFile": "minicon",
    "CFBundlePackageType": "APPL",
    "CFBundleShortVersionString": version,
    "CFBundleVersion": version,
    "LSMinimumSystemVersion": "11.0",
    "NSHighResolutionCapable": True,
    "NSAppTransportSecurity": {"NSAllowsArbitraryLoadsInWebContent": True},
}
with open(sys.argv[1], "wb") as output:
    plistlib.dump(payload, output, sort_keys=True)
PY
  plutil -lint "$dev_contents/Info.plist" >/dev/null
  echo "[build] macOS dev app: $dev_app"
}

build_linux_browser() {
  [ "$(uname -s)" = Linux ] || return 0
  profile="$1"
  helper_profile=release-fast
  [ "$profile" != release ] || helper_profile=release
  cargo build --locked -p minicon-web --features wry-host --bin minicon-web-host --profile "$helper_profile"
  output_root="${CARGO_TARGET_DIR:-$repo_root/target}"
  if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
    output_root="$output_root/$CARGO_BUILD_TARGET"
  fi
  export MINICON_BROWSER_EMBEDDED_HOST="$output_root/$helper_profile/minicon-web-host"
}

if [ "$#" -eq 0 ]; then
  case "$mode" in
    dev) build_linux_browser debug ;;
    release) build_linux_browser release ;;
  esac
fi

case "$mode" in
  release) cargo build --locked --release "$@" ;;
  dev) cargo build --locked "$@"
    if [ "$#" -eq 0 ]; then
      assemble_macos_dev_app
    fi ;;
  check) cargo check --locked --workspace --all-targets "$@" ;;
  # A selected suite uses the same dangerous-test-lint gate as the full run,
  # without compiling or executing unrelated workspace/fork test binaries.
  test-suite) cargo test --locked --config "$test_lints" "$@" ;;
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
    --config "$test_lints" \
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
          --lib) ;; # Already selected by the separately owned fork below.
          --) libtest_args=true; fork_args+=("$argument") ;;
          --features|-F) skip_feature_value=true ;;
          --features=*|-F?*) ;;
          *) fork_args+=("$argument") ;;
        esac
      fi
    done
    cargo test --locked --manifest-path third_party/vt100/Cargo.toml --config "$test_lints" --lib ${fork_args[@]+"${fork_args[@]}"}
    # The shared grammar boundary oracle belongs to the owned VTE lexer.
    cargo test --locked --manifest-path third_party/vte/Cargo.toml --config "$test_lints" --lib ${fork_args[@]+"${fork_args[@]}"} ;;
  *) echo "usage: scripts/build.sh [release|dev|check|test|test-suite] [cargo arguments...]" >&2; exit 2 ;;
esac
