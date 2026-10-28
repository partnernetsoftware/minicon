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
  to `plan-v0.1.22.md` §4 (also archived; see below).
- `plan-0120-detachable-gui-and-display-backends.md` — shipped in 0.1.20.
- `plan-v0.1.20-windows-font-rendering.md` — shipped as 0.1.21.
- `plan-utm-court-extract.md` — the extraction landed; utm-court is its own
  repository and its leftovers are tracked there.
- `plan-runtime-memory-next.md` and the seven `research-*.md` — the host memory
  track, paused since 2026-09-06. The 10 MiB target is carried in
  `plan/plan-carried-debt.md` C4 (claimed but `BLOCKED` by
  `plan/archive/plan-v0.2.1.md`'s `UI` leaf pending a display-capable host); the
  evidence is in `archive/research/`.

Archived with the pre-0.2.2-planning clean-up (2026-09-26):

- `plan-v0.1.22.md` — shipped v0.1.22 (the previous "clear the attic" round).
  Its own carried debt moved to `plan-v0.1.23.md` §0 (also archived) and then
  to `plan/plan-carried-debt.md`, which is current.
- `plan-v0.1.23.md` — shipped v0.1.23. Its ConPTY scrollback question and
  "rules that stay" are historical; nothing here is still open scope.
- `plan-ghcr-toolchain-prebake.md` — scoping settled: cross-compile in this
  toolchain-prebaked container is proven unblocked (run `36114279354`) but
  does not replace the release Mac's local `six-cell-qualify.sh` as the
  routine path. See `prd/PRD_02_27_con_delivery.md` ("Where the bytes come
  from") for how this sits next to the production pipeline.

Archived after the 0.2.4 release (2026-10-10):

- `plan-v0.2.1.md` — shipped v0.2.1. Its remaining `UI` leaf is carried by
  `plan/plan-carried-debt.md` (C3/C4); still cited as evidence history by
  `prd/PRD_02_31_v0_2_horizon.md` and
  `prd/archive/v0.2.1-release-history.md`.
- `plan-v0.2.3.md` — shipped v0.2.3. Its remaining `{HB}`/`{UI-C3}`/`{RELTOOL}`
  leaves were carried into `plan-v0.2.4.md` and onward; still cited by
  `prd/PRD_02_27_con_delivery.md` and
  `prd/archive/v0.2.3-release-history.md`.

Archived with the repo-hygiene pass (2026-10-12):

- `plan-v0.2.5.md` — shipped v0.2.5 (inline tab rename + per-row title line;
  the shared platform crates moved into MiniCon). See
  `prd/archive/v0.2.5-release-history.md`.
- `plan-v0.2.4.md` — shipped v0.2.4. See `prd/archive/v0.2.4-release-history.md`.
- `plan-cross-project-reuse.md` — superseded 2026-10-11 by `plan-v0.2.5.md`
  (the "two products pin each other's crates" premise ended when the platform
  crates moved into MiniCon). Kept as the record of *why* the pin-drift problem
  existed.
- `plan-v0.2.2.md` — the 0.2.2 patch path; `{LP}` closed 2026-09-26 and its
  scope rolled forward to 0.2.4. No release exists at 0.2.2, so there is no
  `v0.2.2-release-history.md`.
- `plan-hostif-v1.md` — the `{HOSTIF}` v1 host-interface. Shipped: the coding
  asks (`minicon --version --json`, `minicon --hostif-handshake`) are
  implemented in `src/cli.rs`; §3 is a release-process contract. Cited by
  `prd/PRD_02_23_minicon.md` and `prd/PRD_02_31_v0_2_horizon.md`.
- `design-frame-channel-experiment.md` — completed decision: the Q1 host-UI
  frame needs no channel (the painter was already a pure slice painter);
  findings for the complete live frame are recorded, not built.

Archived with the 0.2.6 release (2026-10-13):

- `plan-v0.2.6.md` — shipped v0.2.6 (the right-side sent-history panel, its
  header toggle pair, the settings shortcut entry, a repo-hygiene docs pass and a
  Linux test compile fix). See `prd/archive/v0.2.6-release-history.md`.

Archived with the 0.2.7 release (2026-10-13):

- `plan-v0.2.7.md` — shipped v0.2.7 (manual check-for-updates, window-icon
  parity on Windows/X11, `Shift+Enter` sends, and a TLS-provider fix to the
  outbound client that no offline gate could catch). See
  `prd/archive/v0.2.7-release-history.md`. The notes scratchpad (GitHub #5) it
  inherited from 0.2.6 stayed open; it is carried in `PRD.md`, not here.

Archived with the 0.2.8 release (2026-10-14):

- `plan-v0.2.8.md` — shipped v0.2.8: an update can no longer brick an install
  (`{EXECBIT}`/`{ROLLBACK}`/`{BOMB}`/`{SCHEME}` plus `{RECOVER}`/`{SLOGAN}`),
  clipboard image paste on both paste surfaces, and a scrollbar scaled to the
  live scrollback depth. See `prd/archive/v0.2.8-release-history.md`. Its
  "Deferred to 0.2.9" table is what seeded `plan/plan-v0.2.9.md`; read it here
  for the reasoning, not for open scope.

Complete file index, so a reader can tell "listed" from "missing" at a glance —
the per-file text above is the authority, this list is only a map:

- `design-frame-channel-experiment` · `design-osx-x86-64-court-experiment`
  · `design-qvm-false-positive-experiment` · `plan-0.1.9-dual-signed-release`
  · `plan-0.1.10-ui` · `plan-0.1.12-review` · `plan-0.1.13-release-resume`
  · `plan-0.1.16` · `plan-0.1.18`
  · `plan-0120-detachable-gui-and-display-backends` · `plan-cross-project-reuse`
  · `plan-ghcr-toolchain-prebake` · `plan-hostif-v1` · `plan-lazy-font-memory`
  · `plan-macos-developer-id-signing` · `plan-runtime-memory-next`
  · `plan-utm-court-extract` · `plan-v0.1.5` · `plan-v0.1.6`
  · `plan-v0.1.20-windows-font-rendering` · `plan-v0.1.22` · `plan-v0.1.23`
  · `plan-v0.1.24` · `plan-v0.2.0` · `plan-v0.2.1` · `plan-v0.2.2` · `plan-v0.2.3`
  · `plan-v0.2.4` · `plan-v0.2.5` · `plan-v0.2.6` · `plan-v0.2.7` · `plan-v0.2.8`
  · `research-external-residency-next` · `research-frame-lifetime`
  · `research-hello-memory` · `research-minicon-memory` · `research-pixel-host`
  · `research-rss-ledger` · `research-windows-memory`
- Five of those have no per-file entry above — `plan-0.1.10-ui`,
  `plan-0.1.13-release-resume`, `plan-lazy-font-memory`, `plan-v0.1.24` and
  `plan-v0.2.0`. Recorded as a backlog gap rather than left as a silent
  omission; `plan-v0.2.0`'s narrative is owned by
  `prd/PRD_02_31_v0_2_horizon.md`.

Never reuse an archived run as evidence for newer source bytes.

## 2026-10-24 release closeout

- [0.2.26 watch-floor cut](plan-v0.2.26.md): public release evidence is owned
  by [its history](../../prd/archive/v0.2.26-release-history.md); unfinished
  native and observer acceptance stays with PRD 39.01, PRD 25 and PRD 26.01.
- [0.2.22 implementation and parallel trial](plan-v0.2.22.md): public result is
  owned by its release history; local timing, failed controls and earlier scope
  markers remain as observed.
- [0.2.21 goal](goal-0.2.21.md): superseded closeout sequence.
- [0.2.16 proposal](plan-v0.2.16.md): incomplete; PRD 33 retains open obligations.
- [0.2.17 proposal](plan-v0.2.17.md): incomplete; PRD 24.01/34/35 retain open gates.
- [Roadmap snapshot](plan-v0.2.x-product-roadmap-2026-10-24.md) and
  [hygiene audit ledger](plan-repo-hygiene-2026-10-24.md): preserved decisions,
  measurements and rejected alternatives; live paths hold current sequence.

- [Retired tray-era pointer](plan-wry-tray-browser.md) and
  [blocked early NewAgent design](plan-newagent-config.md): historical input;
  current browser and 0.3.x authority stay with PRD 25/31.
- [Generated round view](round-baseline-2026-10-24.md): historical source/cost
  observation. New generated views accompany ignored receipts, not live plans.
