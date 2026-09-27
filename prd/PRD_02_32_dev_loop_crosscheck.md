# Cross-arch dev-test loop for an agent session (`dev-loop-crosscheck`)

Owner of `.github/workflows/dev-loop-crosscheck.yml`. This is a narrower,
separate mechanism from the release pipeline in
`prd/PRD_02_27_con_delivery.md` — read that module for how release evidence,
the six-cell build and GHCR publish work. This module owns only the
recurring dev-loop need: an agent session (typically Linux-only, with no
osx/win host of its own) that must develop a cross-arch fix and prove it
against a real runtime, using nothing beyond default GitHub Actions access.

## Problem and non-goal

A cloud coding session can reach only its own host's architecture directly.
Reproducing or fixing a bug that only shows up on another cell (Windows,
macOS, or even a different Linux ISA) needs a real run on that cell, not a
cross-compiled guess. The existing courts do not fit this recurring need:

- `six-grid-cloud-build.yml` already compiles natively per cell in CI, but is
  wired to the release-evidence receipt/identity path and a GHCR publish that
  requires all six cells and `packages: write` — the wrong shape for "build
  and test one or two cells while iterating on a fix."
- `local-artifact-probe.yml` needs a pre-built bundle behind a release asset,
  which needs release-create/upload rights this kind of session does not
  have (see "Permission walls" below).

Non-goal: this mechanism never becomes a release-evidence path, never
publishes anywhere, and never needs `packages: write` or `contents: write`.
Compiling here is for development feedback only, never a substitute for the
exact-source qualification `prd/PRD_02_27_con_delivery.md` owns.

## Mechanism: `.github/workflows/dev-loop-crosscheck.yml`

Push a fix to `main`, dispatch this workflow, and each requested cell builds
and runs the real test suites natively on its own GitHub-hosted runner —
the same "compile on the cell that will run it" pattern
`six-grid-cloud-build.yml` uses for release, minus its GHCR publish and
receipt/identity bookkeeping. Only `permissions: contents: read`. No GHCR
token, no release, no new tag, no local cross-compile or transfer step.

Inputs: `ref` (defaults to the dispatched branch), `cells` (space-separated;
empty runs the four non-macOS cells — macOS runners bill roughly 10x Linux
and queue longest, so opt in explicitly with `all` or the `osx-*` names when
a macOS-specific question needs it), `test_filter` (a Rust test-name
substring, applied to every suite).

Per cell: `cargo build --locked --workspace --all-targets --target <triple>`,
then each suite is run with `cargo test --locked --target <triple> --test
<name>` (`-p minicon-core` for the crate's own unit tests) — **not** a
guessed path under `target/<triple>/debug/<name>`. Cargo integration/unit
test binaries live hashed under `target/<triple>/debug/deps/`; a workflow
that stats a guessed path and treats "not found" as "nothing to run" reports
a false green with zero suites executed. This is not hypothetical: it is
exactly what happened and was caught below.

Failure or a hang (`cancelled`, not just `failure` — the same gap fixed in
`local-artifact-probe.yml`, see `prd/PRD_02_27_con_delivery.md`) uploads
`probe-logs/*.log` via the same pinned `actions/upload-artifact` SHA used
elsewhere in this repo's workflows.

## Permission walls, disambiguated

Found and confirmed distinct while building this (2026-09-27); recorded here
because the framing had been conflated before being corrected:

1. `workflow_dispatch` on an existing workflow, and reading/downloading
   existing release assets — never blocked. This is why the mechanism above
   works at all.
2. Creating a release, uploading to one, or pushing a new tag — blocked by a
   Claude-Code client-side `[Auto-Mode Bypass]` classifier. A session-type
   policy, not a GitHub permission; not worth retrying through another tool.
3. Direct `ghcr.io` push using a sandbox's own token via `oras` — a genuine
   GitHub-side 401 (the token lacks `packages:write`), distinct from (2) and
   confirmed separately (network reachability to `ghcr.io` itself is fine;
   the challenge is a normal unauthenticated 401, not a proxy/network block).

None of the three gate cross-arch *testing* — only cross-arch *publishing*,
which this mechanism never needed. A future session finding itself blocked
on (2) or (3) while trying to test something should reach for this workflow
instead of trying to work around either wall.

## Validation record

- Run `36312887935` (`lnx-x86_64 lnx-aarch64`, commit `38ddba2`, first real
  dispatch): reported `success` in ~107s, but logs show the test-run step
  never executed a single test — the path-guessing bug above. Caught by
  reading job logs rather than trusting the green checkmark.
- Fixed in commit `da29abf`: `cargo test --target <triple> --test <name>`
  instead of a guessed path.
- Run `36313213803` (same two cells, commit `da29abf`): real execution
  confirmed from the logs — `minicon-core` 95/95, `minicon_blackbox` 28/28
  (including `nonexistent_program_via_dash_e_exits_cleanly_instead_of_hanging`),
  `minicon_control` 14/14 (including the process-group regression test
  `killing_a_process_group_also_kills_what_it_forked` from commit `2e789c4`),
  `minicon_throughput` 1/1, on both `ubuntu-24.04` and `ubuntu-24.04-arm`,
  ~3 minutes per cell, no hang, no `cancelled`. This is also the real-CI
  validation that closed the `minicon_core` process-group-teardown hang fix
  (`2e789c4`) on hosted runners, not only locally.

Full narrative and the owner-correction history that led here:
`plan/plan-carried-debt.md`'s G1 leaf (not archived — this module records
the accepted, standing mechanism; the plan leaf keeps the exploration
history that produced it).
