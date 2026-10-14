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
  **Re-checked 2026-10-14:** `git check-ignore` still returns nothing for both.
  `research/frame-channel/q1-host-ui-frame.png` is **committed evidence** (a
  tracked deliverable, not build output), so an ignore rule would not untrack it
  — the open question is whether it stays tracked at all. Still §Pending 4.
- `[-]` **D — dead code**: the `src/terminal.rs` commented-out Ctrl+wheel block
  was **judged and deleted 2026-10-14** (it had drifted to `:2184-2192`); the
  comment above it already carries the fact, so nothing was lost and the
  `send-wheel --ctrl` test path never went through the disabled branch. The bare
  `#[allow(dead_code)]` convergence is **not** a hygiene edit and moved to
  `plan/archive/plan-v0.2.9.md`: the 2026-10-14 audit counted **56** sites (not the 47
  this ledger first recorded), most are `crates/minicon-platform` per-target
  adapters and `contract/*` fields one OS adapter reads, and turning each into a
  `cfg_attr` is a per-site judgement with a six-cell build behind it.
- `[_]` **E — local / remote hygiene**: local `cargo clean` of `target/debug`
  (≈4.6G, regenerable; keep `target-six/`); the `scripts/lima-court.sh` vs
  `scripts/lib/lima-court.sh` (and the `utm-court` pair) — read before merging,
  they may be a library plus a thin wrapper, not a duplicate; and the three
  stale remote branches
  (`origin/__delete_probe_unused__`, `origin/claude/cloud-vm-mechanism-svppqp`,
  `origin/core/no-unreachable-expect`).
  **Re-checked 2026-10-14:** `.github/workflows/ci-minicon.yml.disabled` is
  **not** stale — it is the documented *parked* workflow (its own header says
  the `.disabled` suffix is what keeps GitHub from loading it, and four PRD/plan
  files cite it as parked), so it stays. The branch deletion is still an owner
  answer (§Pending 2) and was not taken.
- `[_]` **G — third pass, the 0.2.9 planning sweep** (owner-ordered 2026-10-14):
  archived the shipped 0.2.8 plan, closed two stale-information contradictions
  the second pass left, and turned the whole audit into the consolidation track
  of `plan/archive/plan-v0.2.9.md`. See §Third pass below.
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

## Third pass — the 0.2.9 planning sweep (2026-10-14)

Owner-ordered, phrased as *"go through the tidy-up flow … then make the 0.2.9
plan."* Two read-only audits (one on code structure, one on document staleness)
ran first; every line below cites the command or file that shows it. Nothing was
moved or deleted except where it says so.

### Executed

- **Archived the shipped 0.2.8 plan** — the 0.2.8 plan moved into
  `plan/archive/` as `plan/archive/plan-v0.2.8.md`, with all ten live references
  repointed
  (`PRD.md` ×2, `prd/PRD_02_23` ×1, `prd/PRD_02_25` ×4, `prd/PRD_02_32` ×1,
  `src/main.rs` ×1, `src/update.rs` ×1). The `src/main.rs` comment naming the
  old path was why this could not be left to the next reader: the alignment gate
  `referenced_repository_paths_exist` scans `src/*.rs`. `plan/archive/README.md`
  gained the 0.2.8 entry **and** a complete file index (the
  `prd/archive/README.md` pattern), so "listed" and "missing" are now
  distinguishable at a glance; five files with no per-file entry are named as a
  backlog gap rather than left silent.
- **Deleted the retired Ctrl+wheel comment block** in `src/terminal.rs` (batch
  D's first half).
- **Fixed the `Cargo.toml` comment that disagreed with the manifest**: it
  claimed "`autotests` is off, so cargo never built or ran them", but only
  `autobins = false` is set. Corrected to say what is true.
- **`PRD.md`'s two "latest release" claims** — v0.2.7's entry said "is the
  latest public release" while v0.2.8's said the same; v0.2.7 is now "the
  previous public release". The delivery tree and the Mermaid palace gained the
  planned v0.2.9 node.
- **`docs/old-windows.html`**'s `--status` sample is still a real 0.1.2 capture;
  it now says so in a sentence above it rather than being silently read as
  today's output (`prd/PRD_02_27_con_delivery.md` had authorised exactly this).

### Found and re-homed into `plan/archive/plan-v0.2.9.md`

The measured findings that needed more than a hygiene edit. Each is now a leaf
with an invariant, evidence and an order — they are **not** done here:

- **Big modules.** `impl ConApp` is 2037 lines (`src/main.rs:985-3021`),
  `impl ConTerminal` 1861 (`src/terminal.rs:179-2039`), and `src/control.rs` has
  a 338-line `mod native_endpoint_tests` at `:1398-1736`, between live code.
  `src/main.rs` is 5390 lines, not the 5136 this ledger recorded.
- **One fact, several copies.** The shortcut list is hand-written in both
  `src/cli.rs:458-467` and `src/ui.rs:665-727` (three languages) while the
  composer's already lives in one place (`minicon_core::keymap`); the CLI
  command list appears in `control.rs`'s `usage()`, its catalogue and its enum,
  plus an unguarded copy in `src/cli.rs:440-452`.
- **Encapsulation.** `update::State.phase` is written directly from `main.rs`
  (`:2125`) around its own pure `apply()`; `TabNode`'s fields are all `pub`; a
  script found ~73 `pub` items referenced nowhere outside their own file.
- **The platform crate.** 90 declared features, 19 enabled directly, 36 reached
  transitively, **54 with no consumer in this tree**.
- **Naming.** ~200 `agenterm` occurrences remain; some are comments naming the
  wrong repository, some are load-bearing (`AGENTERM_*` env vars, a D-Bus name,
  the diagnostics filename) and must not be renamed blind.
- **Two silent `let _` worth a second look** rather than a comment:
  `terminal.rs:1543` (`--emit-snapshot` write failure) and `terminal.rs:734`
  (the DA1/CPR reply write) — the caller cannot learn either failed.
- **`{SCROLL-DAMAGE}`.** `third_party/vt100`'s `scroll_up`/`scroll_down`
  (`grid.rs:658`, `:683`) call `mark_full()` unconditionally, so every scroll
  damages the whole grid — the reported "flashing". The fix is a parser-contract
  change, and the 0.2.9 plan gates it on its own spec.
- **R1 has a date.** `ubuntu-latest` → Ubuntu 26 begins 2026-10-19. Filed as an
  outside-the-version-number workflow pass in the 0.2.9 plan.

### Checked and correctly placed (2026-10-14)

- `docs/*.html`, `CODE_SIGNING_POLICY.md`, `loader/README.md`,
  `crates/minicon-platform/README.md` and `assets/icon/README.md` still hold
  mechanism a PRD owner also holds, but each has a reason to (a public page, a
  test input pinned by `include_str!`, a published crate `readme`). They are
  §Pending 5-8, not hygiene edits.
- `plan/problem-screenshot-pane-hang.md` stays: its owner is registered
  (`{SCRSHOT-HANG}` in `prd/PRD_02_26_con_control_cli.md`) and its content is
  still-live evidence for an open defect, so archiving it would bury detail, not
  tidy anything.
- `plan/plan-harness-context-engineering.md` (`{CTX}` `[_]`) and
  `plan/plan-newagent-config.md` (owner registered, 0.3.x) stay in `plan/`: both
  are current-horizon registrations, not superseded decisions.
- `.github/release-notes.md`'s template and `release.yml`'s rendering agree on
  the `VERSION` and `OPTIONAL_APE` markers and on the asset names.
- No dangling repository-path link exists in live documents. Four archived files
  have no inbound reference, but two are inside an already-indexed directory
  (`archive/research/*`) and two are named in the new archive index above.

## Recorded, deliberately NOT done (feature-level, not hygiene)

**Re-homed 2026-10-14:** the big-module splits, the platform crate's 90
features and the stale naming are no longer "not done" — they are scoped,
evidenced and ordered in `plan/archive/plan-v0.2.9.md`'s consolidation track
(`{SPLIT}`/`{SEAM}`/`{ENCAP}`/`{FEAT}`/`{NAME}`). The `Cargo.toml` comment that
disagreed with `autotests` **was fixed 2026-10-14** (it claimed `autotests` was
off; only `autobins = false` is set).

Still deliberately not done, and not this ledger's job: harness/mux feature
gating (it changes what the handshake advertises — a product decision, see
`{FEAT}`'s non-goal); `docs/*.html` having no generator; `.claude/skills/`
drifting from the canonical copy (last synced 2026-09-26); performance/memory
quantification.

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
5. `crates/minicon-platform/README.md` — its technical body is published as the
   crate's own `readme` (`Cargo.toml:9`), so it cannot simply be deleted.
   Migrate the platform-support matrix and public-API examples into
   `prd/PRD_02_28_shared_core.md` and leave a pointer, or register the README
   itself as the owner of that reference material?
6. `loader/README.md` — drop the delivery/signing/Defender narrative (owned by
   `prd/PRD_02_27_con_delivery.md`) and keep only the pack/rebuild operator
   steps plus a pointer?
7. `CODE_SIGNING_POLICY.md` — remove the mechanism narrative PRD_02_27 owns,
   keeping the five strings `tests/authenticode_inspector_policy.rs:61-73`
   asserts and the five public links? (The path cannot move — the test reads it
   with `include_str!`.)
8. `assets/icon/README.md` — collapse the window-icon mechanism into
   `prd/PRD_02_25_con_workspace.md` "Host UI", keeping the regenerate steps?

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

## Fourth pass: runner-label hygiene, R1 closed (2026-10-14)

Audited by the 0.2.9 chain's post-release pass, after the PRD frontier had
flagged the 2026-10-19 GitHub migration of `ubuntu-latest` to Ubuntu 26 as a
hard deadline. All floating `-latest` runner labels are now pinned to the
versions they resolve to today, so the 10-19 migration changes nothing for
this repository (11 sites, 6 workflows):

- `ubuntu-latest` → `ubuntu-24.04`: `candidate.yml` ×2, `company-signing.yml`,
  `macos-signing.yml`, `release.yml` ×3, `reputation.yml`,
  `release-smoke-test.yml`
- `windows-latest` → `windows-2025`: `release-smoke-test.yml`
- `macos-latest` → `macos-15`: `release-smoke-test.yml` (matching the
  `macos-15` the release chain already pins in `minicon-com.yml` and
  `macos-signing.yml`)

Pinned earlier the same day, for a different reason (the 2025 image's Defender
signature-update path broke mid-chain — `prd/archive/v0.2.9-release-history.md`):
`defender-ci-scan.yml` → `windows-2022`.

Left open for the owner: the three `ubuntu-slim` sites
(`minicon-com.yml:315`, `company-signing.yml:432`, `six-grid-runtime.yml:242`)
are org-owned custom labels that ran green the same day, so they are not
touched; and the per-cell `matrix.runner` labels are pinned inside each
workflow's matrix and were left as each workflow owns them.

`scripts/product-source-hash.sh` excludes `.github/workflows/`, so this pass
touches no release evidence. All 39 workflow files re-parse as YAML after the
edit.
