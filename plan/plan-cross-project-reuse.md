# MiniCon ↔ AgenTerm: evolving together without stepping on each other

Owner's framing (2026-09-22): the two products now evolve jointly. AgenTerm is
driven by cdx-agenterm, cc-agenterm and bdy-ds4flash; MiniCon by cc-minicon.
Raising abstraction and reuse *across* the two is an important and difficult
problem in its own right. This document is MiniCon's proposal for how, grounded
in what the two repositories actually contain today.

## 1. What is shared, and what is merely similar

**Shared by construction** — MiniCon pins two AgenTerm crates by git rev:

- `agenterm-platform` — OS adapters. MiniCon's features come from
  five dependency blocks, not one: a main block of 13 (`clipboard`, `font`,
  `ime`, `input`, `pty`, `window`, ...) plus `input-inject` (dev),
  `native-pixel-window` (Windows), `runtime`, and `portable-pixel-window`
  (Unix). A consumer check that tests only the 13 would miss exactly the
  per-target modules that broke before.
- `agenterm-ui-core` — host-neutral UI arithmetic (`terminal-selection`; it
  names no `default-features = false`, harmless while its default is empty).
- The vendored forks `vt100` and `softbuffer`, patched to the same rev. A
  check that watches only the two crates misses this class (bdy-ds4flash).
- **A patch-level requirement no feature matrix sees.** `terminal-selection`
  pulls `vt100`, and the col-wrap underflow fix lives only in the fork. Both
  products patch `crates.io`'s `vt100` to it, but a shared crate cannot force
  its consumers to patch; a third consumer that did not would silently lose
  the fix. Record it in the ledger as a consumer requirement.

MiniCon's product crate already uses `agenterm-ui-core` directly (glyph cache,
damage regions, `terminal_selection::{TerminalPoint, normalize_endpoints}`,
with no local copy), so the scrollbar move is a layering choice, not a new
dependency.

**Written twice** — found by probing both trees on 2026-09-22:

| logic | AgenTerm | MiniCon |
| --- | --- | --- |
| scrollbar geometry | `agenterm-ui-core::ScrollbarGeometry` | its own `minicon-core/src/scrollbar.rs` (352 lines) |
| click streak (1/2/3) | `ClickChain` in AgenTerm's frontend selection module, plus a separate Unix composer counter | `minicon-core::click::ClickCounter` (see §7: the rules differ) |
| composer editing rules | **not yet located** -- AgenTerm's composer module is 68 lines; the rules are more likely in its frontend input and interaction modules and `ui_geometry` (bdy-ds4flash) | `minicon-core/src/composer.rs` (1,654 lines) |

**Drift** — MiniCon's pin `5eda1de74` was 16, then 17, then 18 AgenTerm commits
behind `main` within an hour on 2026-09-22 -- which is the point: a drift
figure is only true when computed, so reports compute it rather than quote it. Drift is cheap until it isn't: the
v0.1.20 pin bump had to cross 11 platform commits at once.

## 2. Why this is hard

1. **Asymmetric velocity.** AgenTerm is a super-app, still volatile; MiniCon is
   refined and conservative. A shared crate changes at AgenTerm's speed and is
   judged by MiniCon's standards.
2. **A green check in one repository proves little about the other.** Measured
   before: two clean `cargo check`s of `agenterm-platform` while it did not
   compile under MiniCon's feature set, because AgenTerm's default features omit
   whole modules MiniCon enables (`pty`, the Windows console agent).
3. **Concurrent writers.** Four agents, two repositories, one shared crate. Today
   a file-level claim was made by envelope (`windows/font.rs`); nothing records
   such claims where the next agent will look.
4. **"Similar" is not "the same".** Two scrollbars may differ in a rounding rule
   that one product depends on. Merging on appearance breaks one of them.

## 3. Principles

1. **One owner per shared crate.** `agenterm-platform` and `agenterm-ui-core`
   are owned by the AgenTerm lane. MiniCon changes them as a contributor: the
   change lands in AgenTerm's tree, passes AgenTerm's gates, and the owner is
   told by envelope before MiniCon pins it.
2. **Two consumers, then share.** Logic moves into a shared crate only when both
   products use it. Host-neutral logic both use belongs in `agenterm-ui-core`;
   `minicon-core` keeps what only MiniCon uses.
3. **Behaviour first, then move.** Before merging two implementations, pin the
   behaviour both depend on with tests on the *shared* side, run them against
   both implementations, and only then delete one. A difference found this way
   is a decision for both owners, not a silent pick.
4. **The consumer's build is part of the contract.** A change to a shared crate
   is not done until it compiles and tests under *MiniCon's* feature set too.
5. **Claims live in a file, not in a chat.** Who is changing which shared file,
   and until when, is recorded where the next agent reads before editing.

## 4. Mechanisms

- **A consumer feature-matrix check in AgenTerm.** One gate that builds and
  tests `agenterm-platform` with exactly MiniCon's feature set, per target.
  Cheap — no MiniCon checkout — and it closes the "agenterm is green, minicon
  does not compile" class for good. MiniCon keeps the list in sync (a MiniCon
  test can assert its `Cargo.toml` features equal the list AgenTerm checks).
- **A shared type-name list beside the feature list.** The scrollbar was not
  an abstraction disagreement: it was born twice (AgenTerm 2026-08-11 with
  `agenterm-ui-core`; MiniCon moved its own into `minicon-core` 2026-09-12)
  and converged to within one trailing comma. Features cannot catch that.
  AgenTerm lists the public types its shared crates export; MiniCon keeps a
  text check that it defines none of those names itself (bdy-ds4flash).
- **A shared-seam ledger in AgenTerm** -- landed by cdx-agenterm as the
  shared-seam ledger in its docs (agenterm `1ce19971c`): shared crates, owner,
  the pin MiniCon is observed on, the feature combinations to check, current
  claims. Two repositories cannot change atomically, so each lane updates the
  ledger or its own record in its own commit and says so by envelope.
- **Pin cadence.** MiniCon moves its pin once at the start of each release
  cycle and whenever it needs a shared change, never in the middle of a
  Candidate. The bump commit lists the shared-crate commits it crosses.
- **Envelope protocol** for seam changes: title `seam: <crate>/<area>`, body =
  what changes, which consumer needs it, which tests pin it.

## 5. First migrations, strongest evidence first

Reordered after review by the AgenTerm lane.

1. **Scrollbar geometry.** Pinned on the shared side: agenterm `2e77d3195`
   (in review) carries MiniCon's twelve tests, two of them added after
   negative controls showed the original ten stayed green with the inverse's
   rounding removed or the thumb floor zeroed. One finding for both owners:
   truncate-forward, round-back is exact only above two pixels of travel per
   row. Measured before that: with
   whitespace and comments removed and `ScrollbarRect` read as `Rect`, the two
   implementations differ by one trailing comma -- the logic is identical
   (the earlier "37 against 52 lines" was formatting and doc comments). All 10
   of MiniCon's scrollbar tests (round trip over four maxima, full track,
   narrow and inverted tracks, hit edges) pass when run against
   `agenterm-ui-core` unchanged. So no owner decision is needed on behaviour;
   what remains is to move MiniCon's tests to the shared side (an AgenTerm
   change, claimed in the ledger first), then have MiniCon re-export
   `agenterm-ui-core`'s types and delete its copy. One MiniCon decision rides
   on it: `minicon-core` deliberately has no git dependency today, so the
   re-export belongs in the product crate or `minicon-core` accepts
   `agenterm-ui-core` as its one platform-free dependency.
2. **Click streak.** Both products do have one (§7), but the rules differ in
   four places, so it waits on the owners' decisions D1-D4.
3. **Composer editing rules.** Locate AgenTerm's real rules first.

## 6. How we will know it is working

- Duplicated implementations of the same rule: 3 today, trending to 0.
- Pin drift at the start of each MiniCon cycle: small, and never forced by a
  shared-crate break.
- Zero "green in AgenTerm, broken in MiniCon" events once the feature-matrix
  gate exists.

## 7. Click streak: the behaviour audit (read-only, 2026-09-22)

Asked for by cdx-agenterm before any shared crate is edited, and it changed
the plan: the first proposal named AgenTerm's `pointer_input.rs` as the
counterpart, but that file validates an agent's *explicit* `--count 1..3`; it
is not a human click streak. The real grouping sites are these.

| surface | key | window | 2nd | 3rd | 4th | host specifics |
| --- | --- | --- | --- | --- | --- | --- |
| AgenTerm terminal, Unix (`ClickChain<u64, TerminalPoint>`) | tab id + cell | `multi_click_interval_ms()` = 500 (TODO: read the OS setting per host) | Double if same tab+cell within window | Triple only if the caller armed it -- which it does **only after the double's word selection succeeded** -- and the host hint is true (always on Unix) | Single (chain cleared) | -- |
| AgenTerm terminal, Windows (`ClickChain<String, RemotePoint>`) | tab id + cell | same | same | Triple also requires the host hint `clicks >= 3` | Single | Win32 alternates `WM_LBUTTONDOWN`/`WM_LBUTTONDBLCLK` and has no triple message |
| AgenTerm composer, Unix (`ComposerClick`) | byte offset | 500 | 2 | 3 | **stays 3** (`min(3)`) | -- |
| AgenTerm composer, Windows | native `EDIT` control | OS `GetDoubleClickTime` | OS word select | **none** | -- | OS-owned |
| MiniCon terminal (`ClickCounter<TerminalPoint>`, per session) | cell (the session is the tab) | fixed 500 | 2 | 3 always, even after an empty double | 1 (cycle) | identical on every host |
| MiniCon composer (`ClickCounter<usize>`) | byte offset | fixed 500 | 2 | 3 | 1 (cycle) | identical on every host |

**Divergences, each a decision for both owners, not a silent pick:**

- D1 -- after a double-click on a blank cell (no word), AgenTerm's third click
  is Single; MiniCon's is line select.
- D2 -- a fourth composer click: AgenTerm/Unix stays on line select; MiniCon
  cycles back to a caret.
- D3 -- Windows triple click: AgenTerm follows the OS click count; MiniCon
  synthesises it from time and position on every host.
- D4 -- the window: both use 500 ms today; AgenTerm routes it through one
  host policy function meant to read the OS setting, MiniCon hard-codes it.

**Shared test vectors** -- `(surface id, cell, t ms, word found?, host hint)`
in, stage out. A shared type must pass the rows both products agree on, and
each divergence row names which product expects what:

| # | presses | AgenTerm | MiniCon |
| --- | --- | --- | --- |
| V1 | same spot at 0, 100, 200, 300 | 1 2 3 1 | 1 2 3 1 |
| V2 | second press on another cell | 1 1 | 1 1 |
| V3 | second press after the window | 1 1 | 1 1 |
| V4 | second press exactly at the window edge | 1 2 | 1 2 |
| V5 | same cell, different surface id | 1 1 | 1 1 (separate counters) |
| V6 | double on a blank cell, then third (D1) | 1 2 1 | 1 2 3 |
| V7 | third press with host hint false (D3) | 1 2 1 | 1 2 3 |
| V8 | composer, four presses (D2) | 1 2 3 3 (Unix) | 1 2 3 1 |

**Narrowed on review:** AgenTerm's terminal already cycles on a fourth click
(its Triple branch clears both stages, so the next press is Single), the same
as MiniCon. D2 is therefore only about the Unix composer, and it is first an
inconsistency *inside* AgenTerm (terminal cycles, composer saturates) that has
to be settled before anything is shared. MiniCon's two call sites are in the
product crate: `src/terminal.rs` and `src/main.rs`.

**Recommendation:** the shared type should carry AgenTerm's `ClickChain`
semantics -- surface-id key, arm-on-success, host hint -- because they are the
richer model and MiniCon's is a special case of them (hint always true, arm
always). MiniCon adopting them changes D1 for its users; the composer's fourth
click (D2) needs one answer for both.

**Exclusive files for the migration** (to be claimed in the ledger before
editing): in AgenTerm, a new click module in `agenterm-ui-core` and its export,
the frontend selection module (`ClickChain` becomes a re-export or thin
wrapper), the Unix frontend's terminal classify call and `ComposerClick`, and
the Windows remote frontend's classify call; in MiniCon,
`crates/minicon-core/src/click.rs`, `src/terminal.rs` (`register_click`) and
`src/main.rs` (`register_composer_click`).
