# Dispatch a workflow_dispatch workflow via `gh`, find the run it created,
# wait for it, and print a verdict. Sourced by scripts/ci-build.sh and
# scripts/ci-test.sh so both share one polling implementation instead of two
# copies drifting apart.
#
# Requires: `gh` authenticated against this repo (already the case in this
# environment's preconfigured proxy setup; see AGENTS.md's environment notes
# elsewhere for other hosts).

_ci_dispatch_repo() {
  # `gh repo view --json` (and `gh run list` etc. without --repo) goes
  # through GitHub's GraphQL API, which this environment's `gh` token is not
  # allowed to use (measured directly: `gh repo view --json nameWithOwner`
  # returns "GitHub GraphQL is not available from Claude Code sessions").
  # Every other `gh` call in this file passes `--repo` explicitly and uses
  # the REST-backed subcommands, which do work; this derives owner/repo from
  # the git remote instead of asking `gh`, so nothing here needs GraphQL.
  git remote get-url origin | sed -E 's#^.*[/:]([^/]+/[^/]+)(\.git)?$#\1#'
}

# minicon_ci_dispatch_and_wait <workflow-file> <ref> [field=value ...]
#
# Echoes the run URL to stderr as soon as it is found (so a caller tailing
# this script's output sees progress before the run finishes), waits for
# completion with `gh run watch --exit-status`, and returns that command's
# exit code -- 0 only if the run's conclusion was success.
minicon_ci_dispatch_and_wait() {
  local workflow="$1" ref="$2"
  shift 2
  local -a fields=()
  local field
  for field in "$@"; do
    fields+=(-f "$field")
  done

  local repo since run_id
  repo=$(_ci_dispatch_repo)
  # ISO-8601 with a few seconds of slack: `gh run list --created` filters by
  # creation time, and clock skew between this host and GitHub's API has
  # caused a just-dispatched run to be missed by a `since` timestamp taken
  # right at that instant.
  since=$(date -u -d '-30 seconds' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-30S +%Y-%m-%dT%H:%M:%SZ)

  echo "== dispatching $workflow (ref=$ref) ${fields[*]:-}" >&2
  gh workflow run "$workflow" --repo "$repo" --ref "$ref" "${fields[@]}"

  echo "== locating the run gh workflow run just created" >&2
  run_id=""
  for _ in $(seq 1 20); do
    run_id=$(gh run list --repo "$repo" --workflow "$workflow" \
      --created "$since..*" --json databaseId,createdAt \
      --jq 'sort_by(.createdAt) | last | .databaseId // empty')
    [ -n "$run_id" ] && break
    sleep 3
  done
  if [ -z "$run_id" ]; then
    echo "ci-dispatch: could not find the run $workflow just created" >&2
    return 1
  fi

  echo "== run $run_id: https://github.com/$repo/actions/runs/$run_id" >&2
  minicon_ci_wait_run "$run_id"
}

# minicon_ci_wait_run <run-id>
#
# Waits for a run to finish, with the same 0-only-on-success semantics as
# `gh run watch --exit-status`. On a terminal it uses gh's live view; off a
# terminal (captured logs, an agent session, CI) it polls `gh run view`
# instead, because `gh run watch` reprints the run's whole job list every few
# seconds -- measured at ~317 KB of captured output for one six-cell run,
# mostly repeated snapshots, which buries the one line that matters. The
# poll interval is deliberately coarse; nothing here is latency-critical.
minicon_ci_wait_run() {
  local run_id="$1" repo
  repo=$(_ci_dispatch_repo)
  if [ -t 1 ]; then
    gh run watch "$run_id" --repo "$repo" --exit-status
    return
  fi
  local status conclusion last_status=""
  while :; do
    if ! IFS=$'\t' read -r status conclusion < <(
      gh run view "$run_id" --repo "$repo" --json status,conclusion \
        --jq '[.status, (.conclusion // "-")] | @tsv'
    ); then
      echo "== run $run_id: could not read status (gh failed?)" >&2
      return 1
    fi
    if [ "$status" != "$last_status" ]; then
      echo "== run $run_id: ${status}${conclusion:+ ($conclusion)}" >&2
      last_status="$status"
    fi
    [ "$status" = "completed" ] && break
    sleep 15
  done
  [ "$conclusion" = "success" ]
}

# minicon_ci_dump_failed_logs <run-id>
#
# Prints only the failed step output, which is what a caller needs to act on
# -- the full log for a multi-cell matrix run is too large to be worth
# reading whole.
minicon_ci_dump_failed_logs() {
  local run_id="$1"
  local repo
  repo=$(_ci_dispatch_repo)
  gh run view "$run_id" --repo "$repo" --log-failed
}
