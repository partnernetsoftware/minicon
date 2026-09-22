# Archived MiniCon plans

These files preserve completed release plans and bounded experiments. They are
evidence history, not current task entrypoints. Current truth starts at
`PRD.md`, then the owning `prd/PRD_*.md` module.

- `plan-0.1.16.md` — shipped v0.1.16 (`MiniCon.app` + `install-cli`). Its
  Themes A/B/C were never part of 0.1.16 and are carried by
  `plan/archive/plan-0.1.18.md` P2; do not read them here as open scope.
- `plan-0.1.12-review.md` — completed post-v0.1.11 repo review pass.
- `plan-0.1.9-dual-signed-release.md` — executed runbook for the first
  dual-signed release. The reusable procedure is the
  `run-reputation-and-release` skill, not this file.
- `plan-macos-developer-id-signing.md` — shipped in v0.1.9; the reusable
  procedure is the `sign-macos-artifacts` skill.
- `plan-v0.1.6.md` — shipped v0.1.6 Candidate and Promotion plan.
- `plan-v0.1.5.md` — shipped v0.1.5 Candidate and Promotion plan.
- `design-osx-x86-64-court-experiment.md` — completed decision: routine Intel
  macOS qualification uses real Intel runners; Rosetta is userspace evidence,
  not an Intel-kernel claim.
- `design-qvm-false-positive-experiment.md` — completed decision: the compact
  Windows build shape, not missing resources alone, triggered the observed QVM
  heuristic; the accepted Windows release profile preserves the clean shape.

Archived with v0.1.22's clean-up (2026-09-22):

- `plan-0.1.18.md` — both blockers shipped in 0.1.18; its remaining debt moved
  to `plan/plan-v0.1.22.md` §4.
- `plan-0120-detachable-gui-and-display-backends.md` — shipped in 0.1.20.
- `plan-v0.1.20-windows-font-rendering.md` — shipped as 0.1.21.
- `plan-utm-court-extract.md` — the extraction landed; utm-court is its own
  repository and its leftovers are tracked there.
- `plan-runtime-memory-next.md` and the seven `research-*.md` — the host memory
  track, paused since 2026-09-06. The 10 MiB target is carried in
  `plan/plan-v0.1.22.md` §4; the evidence is in `archive/research/`.

Never reuse an archived run as evidence for newer source bytes.
