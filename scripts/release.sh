#!/usr/bin/env bash
# Drives minicon's candidate -> signing -> Defender reputation -> release
# chain end to end, per .claude/skills/run-reputation-and-release/SKILL.md.
#
# Usage: ./scripts/release.sh <version>   e.g. ./scripts/release.sh 0.2.1
#
# Prerequisites (this script does not set these up):
#   - `gh` authenticated against partnernetsoftware/minicon with permission
#     to dispatch workflows and read run artifacts.
#   - A clean working tree on `main`, already up to date with origin/main.
#   - This script bumps the version, commits, and pushes to `main` itself --
#     do not run it with uncommitted changes you want to keep separate.
#
# The final publish step is interactive: the script runs release.yml as a
# dry run first, then asks you to type the version again before dispatching
# the real, irreversible publish. Nothing publishes without that.
set -euo pipefail

VERSION="${1:?usage: $0 <version>, e.g. $0 0.2.1}"
REPO="partnernetsoftware/minicon"
BRANCH="candidate-src-${VERSION}"

if ! command -v gh >/dev/null 2>&1; then
  echo "release.sh: 'gh' CLI not found; see .claude/skills/run-reputation-and-release/" >&2
  exit 1
fi

log() { printf '\n=== %s ===\n' "$1"; }

# Poll the most recent run of a workflow on a given ref until it finishes.
# Prints the run's database id on stdout; all other output goes to stderr.
wait_for_run() {
  local workflow="$1" ref="$2" since_epoch="$3"
  local run_id status conclusion created_epoch
  echo "waiting for $workflow on $ref ..." >&2
  # gh workflow run is fire-and-forget; give the API a moment to list it.
  sleep 8
  for _ in $(seq 1 5); do
    run_id="$(gh run list --repo "$REPO" --workflow "$workflow" --branch "$ref" \
      --limit 1 --json databaseId,createdAt \
      --jq '.[0].databaseId')" || true
    [ -n "${run_id:-}" ] && break
    sleep 5
  done
  if [ -z "${run_id:-}" ]; then
    echo "release.sh: could not find a dispatched run for $workflow on $ref" >&2
    exit 1
  fi
  created_epoch="$(gh run view "$run_id" --repo "$REPO" --json createdAt --jq '.createdAt' | date -f - +%s 2>/dev/null || echo 0)"
  if [ "$created_epoch" -lt "$since_epoch" ] 2>/dev/null; then
    echo "release.sh: latest $workflow run ($run_id) predates this dispatch; refusing to trust it" >&2
    exit 1
  fi
  echo "tracking run $run_id" >&2
  gh run watch "$run_id" --repo "$REPO" --exit-status >&2
  status="$(gh run view "$run_id" --repo "$REPO" --json status --jq '.status')"
  conclusion="$(gh run view "$run_id" --repo "$REPO" --json conclusion --jq '.conclusion')"
  if [ "$status" != "completed" ] || [ "$conclusion" != "success" ]; then
    echo "release.sh: $workflow run $run_id finished status=$status conclusion=$conclusion" >&2
    exit 1
  fi
  echo "$run_id"
}

now_epoch() { date +%s; }

log "0/8 preflight"
if [ -n "$(git status --porcelain)" ]; then
  echo "release.sh: working tree is not clean; commit or stash first" >&2
  exit 1
fi
git fetch origin main
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
  echo "release.sh: local main is not up to date with origin/main" >&2
  exit 1
fi

log "1/8 bump version to ${VERSION} and push"
CURRENT="$(grep -m1 '^version = "' Cargo.toml | sed -E 's/version = "([^"]+)"/\1/')"
sed -i "0,/^version = \"${CURRENT}\"/{s/^version = \"${CURRENT}\"/version = \"${VERSION}\"/}" Cargo.toml
# Update only the minicon package's own Cargo.lock entry, not dependencies
# that happen to share the old version string.
awk -v old="$CURRENT" -v new="$VERSION" '
  BEGIN { in_minicon=0 }
  /^name = "minicon"$/ { in_minicon=1; print; next }
  in_minicon && /^version = / {
    sub(old, new)
    in_minicon=0
    print
    next
  }
  { print }
' Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock
sed -i "s/\"version\": \"${CURRENT}\",/\"version\": \"${VERSION}\",/" release-policy.json
git add Cargo.toml Cargo.lock release-policy.json
git commit -m "release: bump minicon to ${VERSION}"
git push origin main
SHA="$(git rev-parse HEAD)"
echo "source_sha=${SHA}"

T0="$(now_epoch)"
log "2/8 minicon-com.yml (unsigned one-pack/six-cell)"
gh workflow run minicon-com.yml --repo "$REPO" --ref main
COM_RUN="$(wait_for_run minicon-com.yml main "$T0")"
echo "minicon_com_run_id=${COM_RUN}"

log "3/8 company-signing.yml + macos-signing.yml (parallel, release-eligible)"
T1="$(now_epoch)"
gh workflow run company-signing.yml --repo "$REPO" --ref main \
  -f source_sha="$SHA" -f minicon_com_run_id="$COM_RUN" -f qualification_only=false
gh workflow run macos-signing.yml --repo "$REPO" --ref main \
  -f source_sha="$SHA" -f minicon_com_run_id="$COM_RUN" -f qualification_only=false
WIN_RUN="$(wait_for_run company-signing.yml main "$T1")"
MAC_RUN="$(wait_for_run macos-signing.yml main "$T1")"
echo "company_signing_run_id=${WIN_RUN}"
echo "macos_signing_run_id=${MAC_RUN}"

log "4/8 candidate.yml"
T2="$(now_epoch)"
# release-policy.json signing.mode=required -> upstream_run_id is the
# company-signing run, not the minicon-com run. See
# .claude/skills/run-reputation-and-release/SKILL.md "Gate 0" if this policy
# ever changes to signing.mode=off.
gh workflow run candidate.yml --repo "$REPO" --ref main \
  -f source_sha="$SHA" -f upstream_run_id="$WIN_RUN" -f macos_signing_run_id="$MAC_RUN"
CAND_RUN="$(wait_for_run candidate.yml main "$T2")"
echo "candidate_run_id=${CAND_RUN}"

log "5/8 defender-ci-scan.yml (CI-native Defender court)"
T3="$(now_epoch)"
gh workflow run defender-ci-scan.yml --repo "$REPO" --ref main \
  -f candidate_run_id="$CAND_RUN" -f source_sha="$SHA"
DEFCI_RUN="$(wait_for_run defender-ci-scan.yml main "$T3")"
echo "defender_ci_run_id=${DEFCI_RUN}"

DL_DIR="$(mktemp -d)"
gh run download "$DEFCI_RUN" --repo "$REPO" --dir "$DL_DIR"
QUAL_B64_FILE="$(find "$DL_DIR" -name 'reputation-qualification.b64' | head -1)"
if [ -z "$QUAL_B64_FILE" ]; then
  echo "release.sh: reputation-qualification.b64 not found in run $DEFCI_RUN artifacts" >&2
  exit 1
fi
QUAL_B64="$(cat "$QUAL_B64_FILE")"

log "6/8 pin candidate-src-${VERSION} at ${SHA} and run reputation.yml"
git branch "$BRANCH" "$SHA"
git push origin "$BRANCH"
T4="$(now_epoch)"
gh workflow run reputation.yml --repo "$REPO" --ref "$BRANCH" \
  -f candidate_run_id="$CAND_RUN" -f source_sha="$SHA" -f qualification_base64="$QUAL_B64"
REP_RUN="$(wait_for_run reputation.yml "$BRANCH" "$T4")"
echo "reputation_run_id=${REP_RUN}"

log "7/8 release.yml dry run"
T5="$(now_epoch)"
gh workflow run release.yml --repo "$REPO" --ref "$BRANCH" \
  -f candidate_run_id="$CAND_RUN" -f source_sha="$SHA" -f reputation_run_id="$REP_RUN" \
  -f version="$VERSION" -f confirmation="dry-run-publish-v${VERSION}" -f dry_run=true
wait_for_run release.yml "$BRANCH" "$T5" >/dev/null
echo "dry run succeeded."

echo
echo "About to PUBLISH minicon v${VERSION} for real. This is irreversible."
read -r -p "Type the version (${VERSION}) to confirm, anything else to abort: " CONFIRM
if [ "$CONFIRM" != "$VERSION" ]; then
  echo "release.sh: publish not confirmed; stopping after the dry run." >&2
  echo "The ${BRANCH} branch was left in place -- rerun the release.yml publish step manually, then delete it." >&2
  exit 1
fi

log "8/8 release.yml publish"
T6="$(now_epoch)"
gh workflow run release.yml --repo "$REPO" --ref "$BRANCH" \
  -f candidate_run_id="$CAND_RUN" -f source_sha="$SHA" -f reputation_run_id="$REP_RUN" \
  -f version="$VERSION" -f confirmation="publish-v${VERSION}" -f dry_run=false
wait_for_run release.yml "$BRANCH" "$T6" >/dev/null

git push origin --delete "$BRANCH"
log "done"
echo "minicon v${VERSION} published. Verify the GitHub Release and record the run ids"
echo "(minicon-com=${COM_RUN} company-signing=${WIN_RUN} macos-signing=${MAC_RUN}"
echo " candidate=${CAND_RUN} defender-ci=${DEFCI_RUN} reputation=${REP_RUN})"
echo "in a new prd/archive/v${VERSION}-release-history.md, per AGENTS.md."
