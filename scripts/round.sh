#!/bin/bash
# One development round across the six cells: build here, run each cell where
# it answers fastest, write one receipt.
#
#   scripts/round.sh                       # every cell, default suites
#   scripts/round.sh lnx-x86_64 win-aarch64
#   MINICON_ROUND_SUITES="minicon_core minicon" scripts/round.sh
#
# Routing (measured 2026-09-23, plan/plan-v0.1.23.md §2):
#   osx-aarch64, osx-x86_64  -> this Mac (native, and x86_64 under Rosetta)
#   lnx-*, win-*             -> GitHub runners: they download what was built
#                               here and run it; no checkout, no toolchain,
#                               no build in CI.
# GitHub first, utm-court as the local fallback: a hosted Windows runner has
# a real logged-in desktop (runneradmin, session 2, Active, 1024x768), and
# MiniCon's GUI suites run there -- minicon_control 9/9 in 8.5 s against
# 20 s and a hot Mac in the local court. The court stays for the Defender
# scan, legacy images, offline work and many-iteration debugging.
# The bundle goes through a *tagged prerelease*, deleted afterwards: a draft
# release is not reachable from a job ("release not found"), and the tags
# endpoint can answer with an empty asset list while the release's own
# assets endpoint has the file, so the job fetches by release id.
# A cell whose backend cannot answer is BLOCKED. It is never a silent pass,
# and this script never signs, publishes, or touches the release chain.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

ALL_CELLS="osx-aarch64 osx-x86_64 lnx-x86_64 lnx-aarch64 win-x86_64 win-aarch64"
CELLS="${*:-$ALL_CELLS}"
# minicon_core is host-neutral and safe to run bare on any cell. The binary's
# own `minicon` suite is not: some of its tests want a window or a control
# endpoint and hang when the executable is run outside cargo's harness
# (measured 2026-09-23: two rounds hung there). Name other suites explicitly.
SUITES="${MINICON_ROUND_SUITES:-minicon_core}"
SUITE_TIMEOUT="${MINICON_ROUND_SUITE_TIMEOUT:-120}"
OUT_DIR="${MINICON_ROUND_OUT:-$REPO_ROOT/target-six}"
RECEIPT="$OUT_DIR/round-receipt.json"
LOGS="$OUT_DIR/round-logs"
GH_WORKFLOW="local-artifact-probe.yml"
mkdir -p "$LOGS"

results=""   # cell<TAB>stage<TAB>verdict<TAB>seconds<TAB>detail
record() { results="${results}$1	$2	$3	$4	$5
"; printf '[round] %-12s %-16s %-7s %4ss %s\n' "$1" "$2" "$3" "$4" "$5"; }

target_of() {
  case "$1" in
    osx-aarch64) echo aarch64-apple-darwin ;;
    osx-x86_64) echo x86_64-apple-darwin ;;
    lnx-x86_64) echo x86_64-unknown-linux-gnu ;;
    lnx-aarch64) echo aarch64-unknown-linux-gnu ;;
    win-x86_64) echo x86_64-pc-windows-msvc ;;
    win-aarch64) echo aarch64-pc-windows-msvc ;;
  esac
}
# cargo-zigbuild replaces `build`; cargo-xwin takes it as a subcommand.
builder_of() {
  case "$1" in
    osx-*) echo "cargo build" ;;
    lnx-*) echo "cargo zigbuild" ;;
    win-*) echo "cargo xwin build" ;;
  esac
}
backend_of() {
  case "$1" in
    osx-*) echo local ;;
    *) echo github ;;
  esac
}

# --- A3 pre-flight -----------------------------------------------------------
preflight() {
  local t0=$SECONDS free rosetta_rc
  free="$(df -g /System/Volumes/Data | awk 'NR==2 {print $4}')"
  if [ "${free:-0}" -lt 10 ]; then
    record preflight disk BLOCKED $((SECONDS-t0)) "only ${free}G free"
    return 1
  fi
  case "$CELLS" in
    *osx-x86_64*)
      # A broken Rosetta hangs an x86_64 binary before main; probe it with a
      # deadline rather than discovering it inside a 20 minute cell.
      printf 'int main(void){return 0;}\n' >"$LOGS/rosetta-probe.c"
      if ! clang -arch x86_64 -o "$LOGS/rosetta-probe" "$LOGS/rosetta-probe.c" 2>/dev/null; then
        record preflight rosetta BLOCKED $((SECONDS-t0)) "cannot build an x86_64 probe"
        return 1
      fi
      "$LOGS/rosetta-probe" & local probe=$!
      { sleep 15; kill -9 "$probe" 2>/dev/null; } & local killer=$!
      wait "$probe"; rosetta_rc=$?
      kill "$killer" 2>/dev/null; wait "$killer" 2>/dev/null
      if [ "$rosetta_rc" -ne 0 ]; then
        record preflight rosetta BLOCKED $((SECONDS-t0)) \
          "an x86_64 binary did not run; restart oahd (see memory: rosetta hang)"
        return 1
      fi
      ;;
  esac
  record preflight ok PASS $((SECONDS-t0)) "${free}G free"
  return 0
}

# --- build (B1: clone the previous fingerprint dir, then build incrementally) -
fingerprint() { python3 scripts/source-fingerprint.py | python3 -c 'import json,sys;print(json.load(sys.stdin)["sha256"])'; }
FP="$(fingerprint)"
BUILD_DIR="$OUT_DIR/builds/$FP"

seed_cell_dir() {
  local cell="$1" newest
  [ -d "$BUILD_DIR/$cell" ] && return 0
  newest="$(ls -td "$OUT_DIR"/builds/*/"$cell" 2>/dev/null | grep -v "/$FP/" | head -1)"
  mkdir -p "$BUILD_DIR"
  [ -n "$newest" ] && cp -c -R "$newest" "$BUILD_DIR/" 2>/dev/null
  return 0
}

# The MSVC CRT/SDK is already cached (~/Library/Caches/cargo-xwin/xwin), and
# cargo-xwin only fetches its version manifest when a proxy is configured.
# With no proxy set it builds offline from that cache; measured 2026-09-23,
# a Windows target built in 2 s with the proxy unset after failing on a 503
# through the proxy. Builds run without one; `gh` sets its own.
build_cell() {
  local cell="$1" t0=$SECONDS target builder
  local -x HTTPS_PROXY= HTTP_PROXY= https_proxy= http_proxy=
  unset HTTPS_PROXY HTTP_PROXY https_proxy http_proxy
  target="$(target_of "$cell")"; builder="$(builder_of "$cell")"
  seed_cell_dir "$cell"
  # The seeded dir arrives with a previous round's deps/ in it, so "an
  # executable exists" proves nothing about which source it came from. This
  # stamp is the line between the two: anything this build did not touch is
  # older than it, and `fresh_binary` refuses it. Round 35979574823 shipped a
  # win-x86_64 suite three commits stale and reported two already-fixed tests
  # as live defects; the receipt said PASS on the build that produced it.
  : >"$BUILD_DIR/$cell/.round-build-stamp"
  if CARGO_TARGET_DIR="$BUILD_DIR/$cell" $builder --locked --workspace --all-targets \
      --target "$target" >"$LOGS/$cell-build.log" 2>&1; then
    record "$cell" build PASS $((SECONDS-t0)) ""
    return 0
  fi
  record "$cell" build FAIL $((SECONDS-t0)) "$LOGS/$cell-build.log"
  return 1
}

# A file this round's build did not produce is not this round's answer.
# Prints the path only when it is newer than the build stamp.
fresh_binary() {
  local path="$1" stamp="$BUILD_DIR/$2/.round-build-stamp"
  [ -n "$path" ] && [ -f "$path" ] || return 1
  [ -f "$stamp" ] && [ "$path" -nt "$stamp" ] || return 1
  printf '%s\n' "$path"
}

suite_binary() {
  local cell="$1" suite="$2" target ext=""
  target="$(target_of "$cell")"
  case "$cell" in win-*) ext=.exe ;; esac
  # deps/ also holds object files whose names start the same way, and a seeded
  # dir holds every previous round's hash as well. Newest first, then the
  # stamp decides: alphabetical `head -1` picked by hash, which is a coin toss
  # between this source and last week's.
  ls -t "$BUILD_DIR/$cell/$target/debug/deps/$suite"-*"$ext" 2>/dev/null |
    grep -E "/$suite-[0-9a-f]+${ext:+\\.exe}$" |
    while read -r candidate; do fresh_binary "$candidate" "$cell" && break; done
}

# --- local backend -----------------------------------------------------------
run_local() {
  local cell="$1" suite bin t0
  for suite in $SUITES; do
    t0=$SECONDS
    bin="$(suite_binary "$cell" "$suite")"
    if [ -z "$bin" ]; then
      record "$cell" "$suite" BLOCKED $((SECONDS-t0)) "no test executable from this build"
      continue
    fi
    # Bounded: a suite that wants a desktop can hang forever otherwise.
    "$bin" >"$LOGS/$cell-$suite.log" 2>&1 & local pid=$! rc=0
    { sleep "$SUITE_TIMEOUT"; kill -9 "$pid" 2>/dev/null; } & local killer=$!
    wait "$pid" || rc=$?
    kill "$killer" 2>/dev/null; wait "$killer" 2>/dev/null
    if [ "$rc" -eq 0 ]; then
      record "$cell" "$suite" PASS $((SECONDS-t0)) "$(grep -c '^test .* ok$' "$LOGS/$cell-$suite.log") tests"
    elif [ $((SECONDS-t0)) -ge "$SUITE_TIMEOUT" ]; then
      record "$cell" "$suite" BLOCKED $((SECONDS-t0)) "no verdict within ${SUITE_TIMEOUT}s"
    else
      record "$cell" "$suite" FAIL $((SECONDS-t0)) "$LOGS/$cell-$suite.log"
    fi
  done
}

# --- github backend ----------------------------------------------------------
# Upload the executables built here to a throwaway prerelease, dispatch one job
# per cell that downloads and runs them, then delete the prerelease. A draft
# release is not reachable by tag from a job ("release not found").
run_github() {
  local cells="$1" t0=$SECONDS tag stage bundle cell suite bin run_id
  command -v gh >/dev/null || { for cell in $cells; do record "$cell" github BLOCKED 0 "gh is not installed"; done; return 1; }
  tag="round-$(date -u +%Y%m%d-%H%M%S)"
  bundle="$LOGS/bundle"; rm -rf "$bundle"; mkdir -p "$bundle"
  for cell in $cells; do
    # The product binary rides along, because the black-box suites spawn it
    # rather than linking it. Without it every one of them fails identically
    # on "minicon is missing", which reads like 30 product defects and is one
    # missing file (measured 2026-09-24, run 35978904963).
    local product ext=""
    case "$cell" in win-*) ext=.exe ;; esac
    product="$BUILD_DIR/$cell/$(target_of "$cell")/debug/minicon$ext"
    if fresh_binary "$product" "$cell" >/dev/null; then
      cp "$product" "$bundle/$cell-minicon$ext"
    else
      record "$cell" product BLOCKED 0 "no product binary from this build"
    fi
    for suite in $SUITES; do
      bin="$(suite_binary "$cell" "$suite")"
      [ -n "$bin" ] || { record "$cell" "$suite" BLOCKED 0 "no test executable from this build"; continue; }
      case "$cell" in win-*) cp "$bin" "$bundle/$cell-$suite.exe" ;; *) cp "$bin" "$bundle/$cell-$suite" ;; esac
    done
  done
  [ -n "$(ls -A "$bundle")" ] || return 1
  if ! gh release create "$tag" --prerelease --target main --title "round $tag" \
      --notes "throwaway: executables built on the developer machine" "$bundle"/* >/dev/null 2>&1; then
    for cell in $cells; do record "$cell" upload BLOCKED $((SECONDS-t0)) "release upload failed"; done
    return 1
  fi
  record github upload PASS $((SECONDS-t0)) "$(ls "$bundle" | wc -l | tr -d ' ') files"
  stage=$SECONDS
  if ! gh workflow run "$GH_WORKFLOW" --ref main -f bundle_tag="$tag" \
      -f cells="$(echo $cells)" >/dev/null 2>&1; then
    for cell in $cells; do record "$cell" dispatch BLOCKED $((SECONDS-stage)) "dispatch failed"; done
    gh release delete "$tag" --yes --cleanup-tag >/dev/null 2>&1
    return 1
  fi
  sleep 6
  run_id="$(gh run list --workflow "$GH_WORKFLOW" -L1 --json databaseId -q '.[0].databaseId')"
  # Bound the wait: a queued runner (macos-13 especially) must not hold a
  # round open. Past the bound the cells are BLOCKED, never a pass.
  local watch_deadline=$((SECONDS + ${MINICON_ROUND_GH_TIMEOUT:-300}))
  while [ "$SECONDS" -lt "$watch_deadline" ]; do
    [ "$(gh run view "$run_id" --json status -q .status)" = completed ] && break
    sleep 5
  done
  for cell in $cells; do
    local conclusion
    conclusion="$(gh run view "$run_id" --json jobs -q ".jobs[] | select(.name|contains(\"$cell\")) | .conclusion")"
    case "$conclusion" in
      success) record "$cell" github PASS $((SECONDS-stage)) "run $run_id" ;;
      failure) record "$cell" github FAIL $((SECONDS-stage)) "run $run_id" ;;
      *) record "$cell" github BLOCKED $((SECONDS-stage)) "conclusion=${conclusion:-none} run $run_id" ;;
    esac
  done
  gh release delete "$tag" --yes --cleanup-tag >/dev/null 2>&1
  return 0
}

# --- B2: which cells does this change actually need? -------------------------
# Paths that cannot change a compiled artifact do not need a build cell. The
# receipt says what was skipped and why; nothing is skipped silently, and a
# cell the caller named explicitly is always run.
select_cells() {
  local base changed
  [ -n "${MINICON_ROUND_ALL:-}" ] && { echo "$ALL_CELLS"; return; }
  [ "$#" -gt 0 ] && { echo "$*"; return; }
  base="$(git rev-parse --verify --quiet HEAD 2>/dev/null)" || { echo "$ALL_CELLS"; return; }
  # Tracked changes only: listing untracked files would walk target-six/,
  # which holds tens of gigabytes of build output and takes minutes.
  changed="$(git diff --name-only HEAD -- . ':!target*' 2>/dev/null | sort -u)"
  [ -n "$changed" ] || { echo "$ALL_CELLS"; return; }
  if printf '%s\n' "$changed" | grep -qvE '^(plan/|prd/|docs/|archive/|README|PRD\.md|AGENTS\.md|\.github/)'; then
    echo "$ALL_CELLS"
  else
    echo ""
  fi
}

# --- the round ---------------------------------------------------------------
started=$SECONDS
if [ "$#" -eq 0 ]; then
  CELLS="$(select_cells)"
  if [ -z "$CELLS" ]; then
    record round selection PASS 0 "no compiled artifact can change; documents only"
  fi
fi
preflight || true
github_cells=""
for cell in $CELLS; do
  build_cell "$cell" || continue
  case "$(backend_of "$cell")" in
    local) run_local "$cell" ;;
    github) github_cells="$github_cells $cell" ;;
  esac
done
[ -n "${github_cells// /}" ] && run_github "$github_cells"

python3 - "$RECEIPT" "$FP" "$((SECONDS-started))" "$results" <<'PY'
import json, sys, subprocess
receipt, fingerprint, seconds, raw = sys.argv[1:5]
rows = []
for line in raw.splitlines():
    if not line.strip():
        continue
    cell, stage, verdict, secs, detail = (line.split("\t") + [""] * 5)[:5]
    rows.append({"cell": cell, "stage": stage, "verdict": verdict,
                 "seconds": int(secs or 0), "detail": detail})
tally = {v: sum(1 for r in rows if r["verdict"] == v) for v in ("PASS", "FAIL", "BLOCKED")}
sha = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
json.dump({"kind": "minicon-round", "source_sha": sha,
           "source_fingerprint": fingerprint, "seconds": int(seconds),
           "tally": tally, "stages": rows},
          open(receipt, "w"), indent=2, sort_keys=True)
print(json.dumps(tally, sort_keys=True))
print(f"[round] receipt: {receipt}  total {seconds}s")
PY
case "$results" in *"	FAIL	"*) exit 1 ;; esac
case "$results" in *"	BLOCKED	"*) exit 3 ;; esac
exit 0
