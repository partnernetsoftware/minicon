# v0.1.22 — clear the attic, then make the code say what it does

Owner's brief (2026-09-22): take stock of the directories, files and documents;
archive what is old and no longer used; then raise the code's abstraction and
reuse. No new user-facing feature is the goal of this release; clarity is.

## 1. Inventory (at v0.1.21, `e24cfaf`)

| tree | files | lines | state |
| --- | ---: | ---: | --- |
| `src/` | 24 | 23,131 | product; `main.rs` alone is 7,947 |
| `crates/minicon-core` | 7 | 3,560 | host-neutral core (composer, json, numeric, scrollbar, tree) |
| `research/` | 163 | 28,667 | **larger than `src/`**; one subtree live, eight dead |
| `prd/` | 29 | 5,976 | live PRD modules + `prd/archive/` release histories |
| `docs/` | 14 | 5,370 | public site (index, control CLI, old Windows) |
| `plan/` | 24 | 3,943 | 12 archived + 12 at top level, most of them finished |
| `scripts/` | 38 | 3,959 | build, six-cell, six-grid, self-tests |
| `lab/` | 23 | 3,294 | two finished experiments |
| `tests/` | 9 | 7,491 | integration and policy tests |
| `ui/`, `assets/`, `vendor/`, `reference/` | 16 | 3,738 | live |

### What is live in `research/`

Only `loader/` is referenced — by five release workflows,
three policy tests, `AGENTS.md` and `README.md`. It is not research: it holds
the `minicon.com` APE loader source, its packaging, the signing and Candidate
receipt tooling, and the Defender and reputation courts. Five of its 34 files
are unreferenced and finished -- the SignPath application (SignPath declined
it; not a route), the v0.1.3 Candidate plan, and three superseded probes. They
are now in `archive/research/minicon-com-loader-retired/`, whose index in
`archive/README.md` names each.

The other eight subtrees — `frame-lifetime`, `hello-memory`, `minicon-memory`,
`osx-x86-64-court`, `pixel-platform`, `rss-ledger`, `screenshot-memory`,
`windows-memory` (129 files) — have not changed since 2026-09-05/06 and nothing
references them. They are the evidence behind the memory research documents in
`plan/research-*.md`.

### What is finished in `plan/`

- `plan/archive/plan-0.1.18.md` — both blockers shipped in 0.1.18; carries only debt, which
  moves to §4 below.
- `plan/archive/plan-0120-detachable-gui-and-display-backends.md` — shipped in 0.1.20.
- `plan/archive/plan-v0.1.20-windows-font-rendering.md` — shipped as 0.1.21.
- `plan/archive/plan-utm-court-extract.md` — the extraction landed; utm-court is its own
  repository and its leftovers belong there.
- `plan/archive/plan-runtime-memory-next.md` and the seven `research-*.md` — the memory track
  has not moved since 2026-09-06. The target (idle one-tab host RSS toward
  10 MiB) stays in §4; the working documents are history.

### What is finished in `lab/`

- `hello-window/` — the baseline for the 360 QVM false-positive decision, whose
  design document is already archived with its conclusion.
- `tinygui/` — the empty-pixel-window memory floor, recorded in
  `archive/lab/tinygui/RESULTS.md`.

## 2. Archive (this release, first)

| from | to |
| --- | --- |
| `research/<8 dead subtrees>/` | `archive/research/<same>/` |
| the 5 finished loader files | `archive/research/minicon-com-loader-retired/` |
| `lab/hello-window/`, `lab/tinygui/`, the lab index | `archive/lab/` |
| the finished plans and research documents above | `plan/archive/` |

`archive/README.md` indexes what each item decided, so a reader can find the
conclusion without reading the evidence. Moves are `git mv`, so history and
blame follow. Every path that pointed at a moved file is rewritten in the same
commit; a link that still resolves to nothing is a bug.

Rule for what goes in `archive/`: finished, referenced by nothing live, and
worth keeping for its conclusion. Anything referenced by a workflow, test,
script or build file is live by definition, whatever its directory is called.

## 3. Abstraction and reuse

Ordered so each step is a pure change with the existing test net as its proof,
and each is committed and verified on its own.

1. **One multi-click counter.** The terminal counts clicks by cell
   (`last_click`, `register_click`) and the composer by byte offset
   (`composer_last_click`) with the same window and the same 1/2/3 rule. One
   generic `ClickCounter<K>` in `minicon-core` replaces both, with the rule
   tested once.
2. **`ConTerminal` gets its own module.** Its struct and two `impl` blocks are
   ~2,350 lines of `main.rs`; they move to `src/terminal.rs` unchanged, then
   split by concern: pointer and selection, keyboard and IME input, PTY I/O.
3. **`main.rs` tests move beside what they test.** 2,104 lines (26% of the
   file) are one inline `mod tests`; after step 2 most belong to the terminal
   module.
4. **The release tooling leaves `research/`.** Split
   `loader/` into `loader/` (the `minicon.com` loader
   source and its packaging) and `release/` (signing receipts, Candidate bundle,
   CI control, Defender and reputation courts), updating the workflows, policy
   tests, `AGENTS.md` and `README.md` together. Verified by the policy tests
   and a `minicon-com.yml` build before the next Candidate depends on it.
5. **What minicon and agenterm should share**, re-assessed after steps 1-4: the
   shared seam is `agenterm-ui-core`; candidates are host-neutral pieces now in
   `minicon-core`. Nothing moves across repositories without a second consumer.

## Progress

| step | commit | result |
| --- | --- | --- |
| archive | `7329412` | 182 files moved; `research/` 163 -> 29 files, `lab/` gone |
| 1 click counter | `b141749` | `minicon_core::click::ClickCounter<K>`, rule tested once |
| 2 terminal module | `c2adb15` | `ConTerminal` -> `src/terminal.rs` (2,326 lines) |
| 3 tests beside code | `2c12755` | 42 terminal tests + 5 helpers moved |
| workflow path invariant | `15517ee` | `every_path_a_workflow_names_exists` |
| 4 release tooling | `ec50ba7` | `loader/` + `release/`; `research/` gone; 7 depth-relative root lookups fixed; `minicon-com.yml` 35678087514 green on it |
| 2b narrow `ConTerminal` | `69a4e4e`, `27941f7` | 9 fields private (compiler-driven: all private, re-expose what fails); `SessionSeed` and startup sizing moved into `src/terminal.rs` |
| 5 shared seam: scrollbar | `84ffa57` | local copy deleted, `agenterm-ui-core` used directly, pin `6ae0f9bb6`; 12 parity tests live in agenterm (`2e77d3195`) |
| consumer matrix | `93e4c37` | MiniCon pins its five `agenterm-platform` feature blocks; AgenTerm builds the same sets in pre-push |

`main.rs`: 7,947 lines at 0.1.21 -> 4,601.

Still open in this section: the remaining `pub(super)` fields are read by the
host painters and control dispatch; each needs a method that says what is
asked, not a visibility flip. Click streak sharing waits on the owners'
decisions (fourth composer click; arm-on-success), recorded in
`plan/plan-cross-project-reuse.md` §7.

## 4. Carried debt and backlog

- `capture-pane --scrollback N` — decide the semantics (cross-screen stitching,
  viewport restore) before implementing. (from 0.1.18 P1)
- A black-box test that paste and Enter arrive in two separate `read()`s — the
  part that regressed. (from 0.1.18 P1)
- Box-drawing glyphs from cell geometry: Consolas leaves a 1 px gap at 12 px.
  (from 0.1.21)
- Idle one-tab host RSS toward 10 MiB. Paused since 2026-09-06; the evidence is
  in `archive/research/` and `plan/archive/research-*.md`. (from the memory
  track)
- The interactive court presents no frames, so real pointer events and pixel
  comparison remain out of reach. (from 0.1.18 P2)

- **Windows ARM court gaps that predate 0.1.22.** With the transfer timeout
  raised to 180 s the full Windows suite runs in the UTM court for the first
  time. v0.1.21 (test-only fix applied) and 0.1.22 fail the same four:
  `minicon_control` `a_host_whose_program_cannot_be_spawned_dies_and_says_why`,
  `a_new_tab_that_cannot_start_is_a_notice_not_an_exit`,
  `gui_control_surface_isolated_multitab_black_box`, and `minicon_throughput`
  (`pty_drained_bytes` a few KB against the 32 MiB payload). Not regressions;
  each needs its own diagnosis. The court's QGA file copy runs at ~240 KB/s,
  which is why these were never reached before.

## Verification for every step

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` on the host
and the Windows target, `cargo test`, `./scripts/selftest.sh`, and the local
six-cell before any push a release will depend on.
