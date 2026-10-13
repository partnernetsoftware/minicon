# plan-repo-hygiene (三理)

Owner-ordered 2026-10-12. This is the durable ledger for the repo-wide
"梳理 / 整理 / 清理" pass: it records what was done, what remains, and what was
deliberately **not** done (so a later session does not re-derive it). It is not a
release plan; version sequencing lives in `plan/plan-v0.2.6.md`.

## Batches

- `[v]` **A — archiving + markers** (commit `905bc84`): archived six
  completed/superseded plans into `plan/archive/` (`plan-v0.2.5`, `plan-v0.2.4`,
  `plan-v0.2.2`, `plan-cross-project-reuse`, `plan-hostif-v1`,
  `design-frame-channel-experiment`), repointed inbound links in `PRD.md`,
  `prd/PRD_02_23`, `prd/PRD_02_28`, `prd/PRD_02_31` and `plan-carried-debt`,
  updated `plan/archive/README.md`, gave explicit non-goals their own `[no]`
  marker so `[-]` means partial/in-progress only, and fixed a `[✓]` and an
  expanded home path in `prd/PRD_02_31_v0_2_horizon.md`.
- `[-]` **B — ledger + registration**: this file; the `PRD.md` delivery tree
  still stops at v0.2.1 and needs v0.2.3/4/5 added; `problem-screenshot-pane-hang`
  is an orphan (no PRD reference) and needs an owner in `prd/PRD_02_26` or an
  archive; GitHub issues #3/#4/#5 have no PRD/plan registration.
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
