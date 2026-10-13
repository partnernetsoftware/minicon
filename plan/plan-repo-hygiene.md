# plan-repo-hygiene (三理)

Owner-ordered 2026-10-12. This is the durable ledger for the repo-wide
"梳理 / 整理 / 清理" pass: it records what was done, what remains, and what was
deliberately **not** done (so a later session does not re-derive it). It is not a
release plan; version sequencing lives in `plan/archive/plan-v0.2.6.md`.

## Batches

- `[v]` **A — archiving + markers** (commit `905bc84`): archived six
  completed/superseded plans into `plan/archive/` (`plan-v0.2.5`, `plan-v0.2.4`,
  `plan-v0.2.2`, `plan-cross-project-reuse`, `plan-hostif-v1`,
  `design-frame-channel-experiment`), repointed inbound links in `PRD.md`,
  `prd/PRD_02_23`, `prd/PRD_02_28`, `prd/PRD_02_31` and `plan-carried-debt`,
  updated `plan/archive/README.md`, gave explicit non-goals their own `[no]`
  marker so `[-]` means partial/in-progress only, and fixed a `[✓]` and an
  expanded home path in `prd/PRD_02_31_v0_2_horizon.md`.
- `[v]` **B — ledger + registration**: this file; the `PRD.md` delivery tree
  stopped at v0.2.1 and needed v0.2.2–v0.2.6 added; `problem-screenshot-pane-hang`
  was an orphan (no PRD reference) and needed an owner in `prd/PRD_02_26` or an
  archive; GitHub issues #3/#4/#5 had no PRD/plan registration. **Closed
  2026-10-13** by the second pass below: the tree carries v0.2.2–v0.2.6 with
  history pointers, `{SCRSHOT-HANG}` is registered in `prd/PRD_02_26_con_control_cli.md`,
  and #3/#4/#5 are owned by `prd/PRD_02_25_con_workspace.md` and
  `plan/archive/plan-v0.2.7.md`.
- `[_]` **C — `.gitignore` real gaps**: the audit's "ignore six directories" was
  **wrong** — `vendor/`, `third_party/`, `prd/`, `release/`, `archive/` are
  deliberately tracked (vendored deps, docs, signing scripts). The only real
  gaps are research output (`research/**/*.png`) and `cargo vendor` residue
  (`third_party/*/Cargo.toml.orig`). Needs owner answers (see §Pending).
- `[_]` **D — dead code**: `src/terminal.rs:2167-2173` is a commented-out block
  whose own comment says it was driven by `send-wheel --ctrl` — judge whether it
  is a stale reference or a deliberately disabled path before touching it; and
  converge the bare `#[allow(dead_code)]` sites (`src/harness.rs:519`,
  `crates/minicon-platform/src/console_line_editor.rs:214,225`,
  `…/windows/process_spawn.rs:64,113,119`,
  `…/linux/accessibility_tree.rs:3875,3893,3910`,
  `…/macos/accessibility_tree.rs:2991`, `…/macos/foreign_windows.rs:697`).
  Needs `cargo fmt` + `clippy -D warnings` + `./scripts/build.sh test`.
- `[_]` **E — local / remote hygiene**: local `cargo clean` of `target/debug`
  (≈4.6G, regenerable; keep `target-six/`); `.github/workflows/ci-minicon.yml.disabled`;
  the `scripts/lima-court.sh` vs `scripts/lib/lima-court.sh` (and the `utm-court`
  pair) — read before merging, they may be a library plus a thin wrapper, not a
  duplicate; and the three stale remote branches
  (`origin/__delete_probe_unused__`, `origin/claude/cloud-vm-mechanism-svppqp`,
  `origin/core/no-unreachable-expect`).
- `[-]` **F — second pass, after the PRD-series fold** (2026-10-13, owner-ordered
  re-review): audit the remaining non-PRD markdown for (a) facts that belong to a
  `prd/PRD_*.md` owner and are therefore a second copy, and (b) stale or dead
  content to archive. Findings and actions are in §Second pass below; every item
  is evidence-backed. Nothing was moved or deleted by this pass.

## Second pass — non-PRD markdown audit (2026-10-13)

Every tracked `.md` outside `target/` was classified against two questions: does a
`prd/PRD_*.md` owner already own these facts (so this file is a second copy), and
is the content still true. Each line below cites the command or file that shows
it; nothing here was moved or deleted by this pass.

### Merge into the PRD series — facts that already have an owner

- `crates/minicon-platform/README.md` — 360 lines, **zero mentions of MiniCon**.
  Still "`minicon-platform` is AgenTerm's reusable Rust boundary",
  `Consumers: **agenterm** (workspace path dependency)`, and a
  `git = "…/agenterm.git"` pin example. That ownership ended 2026-10-10/11
  (`prd/PRD_02_28_shared_core.md`, "Stage 3": the crate is now this repo's
  `crates/minicon-platform` and `agenterm` depends on MiniCon). Owner of the
  facts: PRD_02_28. Action: keep only what an operator acts on (build, feature
  flags, how `src/` consumes it) plus a pointer; anything not already stated in
  PRD_02_28 moves there.
- `loader/README.md` — 181 lines; facts belong to
  `prd/PRD_02_27_con_delivery.md`, which already cites
  `loader/rebuild-payloads.sh`, `loader/pack.sh` and
  `loader/write-size-report.py`. Its header still says "Exploration only", which
  contradicts `minicon.com` being a shipped optional release asset (listed in
  `.github/release-notes.md`'s Downloads table, gated by `release-policy.json`).
  Action: keep the
  pack/rebuild operator steps, drop the duplicated payload/profile narrative,
  fix the stale header line.
- `CODE_SIGNING_POLICY.md` — **not archivable**: public policy, linked from
  `README.zh.md`, `README.en.md` and `docs/index.html`, and read as a test input
  by `tests/authenticode_inspector_policy.rs` (`include_str!`). The signing
  mechanism it restates is owned by `prd/PRD_02_27_con_delivery.md`. Action
  (owner call — public copy): keep policy + pointer, remove the mechanism
  narrative PRD_02_27 owns. The path is pinned by the test and the public links,
  so the file does not move.
- `assets/icon/README.md` — the window-icon mechanism (Windows PE resource via
  `build.rs`; X11/macOS fed the decoded `.ico` pixels) is product behaviour. It
  was re-checked after commit `aeb1a37` ("give the window the product icon on
  every host") landed its own edits here plus owner and scope registration
  (`prd/PRD_02_25_con_workspace.md` "Host UI", `plan/archive/plan-v0.2.7.md`), so the
  remaining question is only the standing one: is this file the operator note
  with PRD_02_25 owning the mechanism, or does a fact here have no owner?
  No action needed beyond that answer.

### Stale, dead or unregistered — archive, fix or register

- `.github/release-notes.md` — the worst one and user-visible. `release.yml:234`
  uses this file verbatim as the Release body, and its Highlights stop at
  v0.1.26: **no 0.2.x entry exists**. Verified against the published release
  (`gh release view v0.2.6`): the v0.2.6 body still announces a "two-tool header
  (New terminal + Settings)" — the header has been four tools since 0.2.6 — and
  omits sent-history (this release's own headline), tab rename (0.2.5) and
  mux/harness (0.2.0). v0.2.0–v0.2.6 all published with it. Action: give the
  per-version facts one owner — either update the file as a release step
  (PRD_02_27) or reduce it to a short template (product blurb + Downloads +
  verify + links) and let `prd/archive/*-release-history.md` carry the version
  facts. Not an archive job either way.
- `PRD.md`'s delivery tree still stops at v0.2.1 — confirmed: `v0.2.2`,
  `v0.2.3` and `v0.2.4` appear **nowhere** in `PRD.md` (grep count 0), and
  v0.2.5/v0.2.6 only under "Current frontier". Also **v0.2.2 is the one 0.2.x
  release with no v0.2.2 release-history doc under `prd/archive/`**; its record lives
  only in `prd/PRD_02_31_v0_2_horizon.md` and `plan/archive/plan-v0.2.2.md`.
  Action: extend the tree; decide whether v0.2.2 gets a history doc or is
  explicitly recorded as folded into PRD_02_31.
- `plan/problem-screenshot-pane-hang.md` — still an orphan: its only references
  are an **archived** plan (`plan/archive/plan-v0.2.6.md`) and this ledger.
  Action (as batch B already recorded): name an owner in
  `prd/PRD_02_26_con_control_cli.md` or archive it.
- `docs/*.html` (public site: `index.html`, `control-cli.html`,
  `old-windows.html`) — **no PRD or plan references any of the three**, so the
  site has no owner in the series. `old-windows.html` has not changed since
  2026-09-29 and its sample output is `minicon 0.1.2`. Action: name the owner
  (PRD_02_27 for the download/release facts, PRD_02_26 for the control-CLI page)
  and state the update trigger; fix or date the old-Windows sample. Related, and
  already recorded as deliberately not done: the pages have no generator.
- `README.zh.md` / `README.en.md` — their shortcut tables lack `Ctrl+Shift+H`
  (the sent-history panel, shipped 0.2.6) while the in-app list in `src/ui.rs`
  carries it in all three languages, so the mirror drifted within one release.
  The release-notes claim that the key handler, `--help` and the README "can no
  longer drift apart" is therefore not a guarantee (see the release-notes item).
- `.gitignore`, batch C's two real gaps, still open: `research/**/*.png`
  (currently `research/frame-channel/q1-host-ui-frame.png`, tracked) and
  `third_party/*/Cargo.toml.orig` — `git check-ignore` returns nothing for both.

### Uncovered while auditing: v0.2.1's public Release does not resolve

- `prd/archive/v0.2.1-release-history.md` records a publication with a public
  Release URL, and the tag `v0.2.1` still exists at the recorded source
  (`1796876f76ffabb74426d95a7dcbe826a8db41b7`) — but **no Release object
  resolves**: `gh release view v0.2.1` and
  `repos/…/releases/tags/v0.2.1` return "not found", and v0.2.1 is absent from
  the release listing that carries v0.2.0 and v0.2.2. The chain's last step
  creates the Release as a draft and then publishes it, so a draft left
  unpublished (or later removed) fits what is visible. This is **not** fixed
  here: `prd/archive/` is frozen record, and only the owner can decide whether
  to re-publish from the existing bytes or amend the claim. Flagged in
  `prd/PRD_02_31_v0_2_horizon.md` (its owner) under the 0.2.1 section.
- Also found: **v0.2.2 published** (2026-09-27) and is the only 0.2.x release
  with no release-history document; its record is `prd/PRD_02_31_v0_2_horizon.md`
  plus `plan/archive/plan-v0.2.2.md`.

### Executed 2026-10-13

- `.github/release-notes.md` reduced to a template (product blurb → pointer to
  the release histories → Downloads → signing pointer → verify → older Windows →
  reporting), with the stale per-version Highlight list removed. Owner and the
  choice are recorded in `prd/PRD_02_27_con_delivery.md`
  ("The public Release body is a template, not a changelog").
- `crates/minicon-platform/README.md` framing corrected: it is MiniCon's own
  crate (not AgenTerm's), `agenterm` consumes it from MiniCon, the dependency
  example points at `partnernetsoftware/minicon`, the feature list is deferred to
  `Cargo.toml`, and the owner is named. The technical reference body
  (platform-support matrix, public-API examples, console-interrupt and
  virtualization contracts) was **left in place** — folding it into PRD_02_28 is
  a content migration that needs owner review, not a hygiene edit.
- `loader/README.md` header corrected: it packs a shipped optional asset, not an
  exploration, and delivery facts are pointed at PRD_02_27.
- `PRD.md` delivery tree extended with v0.2.2–v0.2.6 and their history pointers
  (batch B's open item).
- `plan/problem-screenshot-pane-hang.md` given an owner: a `[-]` entry in
  `prd/PRD_02_26_con_control_cli.md` ("Transport and bounds", `{SCRSHOT-HANG}`)
  that explicitly does not claim it fixed.
- `docs/` site ownership registered: `prd/PRD_02_27_con_delivery.md` for the
  download/release pages, `prd/PRD_02_26_con_control_cli.md` for the
  control-CLI page, both with an update trigger.
- `README.zh.md` / `README.en.md`: the missing `Ctrl+Shift+H` row and a short
  sent-history-panel bullet.
- Still open, deliberately: the `.gitignore` pair (an ignore rule would not
  untrack `research/frame-channel/q1-host-ui-frame.png`, which is committed
  evidence, so it stays with batch C's pending owner answers), the v0.2.1
  Release question above, and the `docs/` pages having no generator.

### Checked and correctly placed — no action

- `prd/evidence/PRD_02_27_runtime_memory.md` — an intentional evidence appendix
  with a named owner and inbound links; the pattern to keep, not a duplicate.
- `vendor/linux/README.md`, `plan/round-baseline.md` (generated by
  `scripts/render-round-baseline.py`), `AGENTS.md`, `README.md` (a 14-line
  pointer), `plan/plan-alt-screen-resize.md`, `plan/plan-carried-debt.md`,
  `plan/plan-harness-context-engineering.md`, `plan/plan-newagent-config.md`,
  `archive/README.md`, `plan/archive/README.md`, `prd/archive/README.md` — each
  already owned or already an index.
- `.claude/skills/` and the vendored `third_party/` forks' upstream README and
  CHANGELOG files
  — vendored copies; their drift against the canonical skill copy is already
  recorded under "Recorded, deliberately NOT done".

## Recorded, deliberately NOT done (feature-level, not hygiene)

`src/main.rs` (5136 lines) and the other large modules' splits; the platform
crate's 90 features; the pinned `agenterm` crate drift and its stale comments;
the `Cargo.toml` comment that disagrees with `autotests`; harness/mux gating
gaps; `docs/*.html` having no generator; `.claude/skills/` drifting from the
canonical copy (last synced 2026-09-26); performance/memory quantification.

## Observed during this pass

`controlled_wheel_moves_the_real_scrollback_offset_up_then_down` fails on this
Windows host **on the pristine tree** (verified by stashing this pass's changes
and re-running): five wheel-up notches move one line, not five. It is
pre-existing, not caused by this pass, and is recorded here rather than fixed.

Found while cross-checking the right-side panel on Linux (`cargo zigbuild
--workspace --all-targets --target x86_64-unknown-linux-gnu`): the `mod tests` in
`crates/minicon-platform/src/adapters/linux/x11_clipboard.rs` used bare
`PLAIN_UTF8_TYPE` and `Duration` without importing them, so the Linux `lib test`
target never compiled. Windows-only development never reached the file. Fixed in
this pass (added the two imports); the Linux workspace now compiles clean.
macOS was not runtime-checked here — it needs the osxcross + Apple SDK toolchain
`scripts/six-cell-qualify.sh` uses, which this Windows host lacks.

## Pending owner answers

1. `archive/research/frame-lifetime/external-results.json` (864K, the largest
   tracked file) — keep or move out? And confirm `vendor/linux/*.so*` (356K) is
   an intentional runtime bundle.
2. The three stale remote branches — delete?
3. Is `cargo clean` wanted now (it forces a full recompile)?
4. `research/` ignore granularity: outputs only, or the whole tree?

## Guard

The alignment test `referenced_repository_paths_exist` fails on any doc or
source comment naming a repo path that does not exist — it caught three dangling
references this pass left behind: two archived plans still cited from `src/cli.rs`
and `src/main.rs`, and this ledger's own path before the file existed. Re-run
`./scripts/build.sh test` after any move.

## Rollback

Docs/ignore: `git revert` / `git checkout -- <path>`. `git rm --cached`:
`git reset <path>` (the file stays in the working tree). `cargo clean` is
regenerable. Record branch SHAs before deleting them.
