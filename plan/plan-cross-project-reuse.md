# MiniCon ↔ AgenTerm: evolving together without stepping on each other

Owner's framing (2026-09-22): the two products now evolve jointly. AgenTerm is
driven by cdx-agenterm, cc-agenterm and bdy-ds4flash; MiniCon by cc-minicon.
Raising abstraction and reuse *across* the two is an important and difficult
problem in its own right. This document is MiniCon's proposal for how, grounded
in what the two repositories actually contain today.

## 1. What is shared, and what is merely similar

**Shared by construction** — MiniCon pins two AgenTerm crates by git rev:

- `agenterm-platform` — OS adapters. MiniCon enables 13 of its features
  (`clipboard`, `font`, `ime`, `input`, `pty`, `window`, ...).
- `agenterm-ui-core` — host-neutral UI arithmetic (`terminal-selection`).

**Written twice** — found by probing both trees on 2026-09-22:

| logic | AgenTerm | MiniCon |
| --- | --- | --- |
| scrollbar geometry | `agenterm-ui-core::ScrollbarGeometry` | its own `minicon-core/src/scrollbar.rs` (352 lines) |
| click streak (1/2/3) | `src/frontend/pointer_input.rs` | `minicon-core::click::ClickCounter` |
| composer editing rules | `src/frontend/composer`, `ui_geometry` | `minicon-core/src/composer.rs` (1,654 lines) |

**Drift** — MiniCon's pin `5eda1de74` is 16 AgenTerm commits behind `main`
(none touching the shared crates yet). Drift is cheap until it isn't: the
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
- **A shared-seam ledger in AgenTerm** (a proposed shared-seam document under its docs directory): the shared crates, their owner, the
  current claims (file, agent, purpose, date), and the pin MiniCon is on. Both
  lanes update it in the same commit as the change.
- **Pin cadence.** MiniCon moves its pin once at the start of each release
  cycle and whenever it needs a shared change, never in the middle of a
  Candidate. The bump commit lists the shared-crate commits it crosses.
- **Envelope protocol** for seam changes: title `seam: <crate>/<area>`, body =
  what changes, which consumer needs it, which tests pin it.

## 5. First migrations, smallest first

1. **Click streak → `agenterm-ui-core`.** Smallest, both products have it, and
   MiniCon's version is already a pure, tested, generic type. Move
   `ClickCounter<K>` into `agenterm-ui-core`; AgenTerm's
   `pointer_input.rs` and MiniCon both use it; delete both copies.
2. **Scrollbar geometry → one implementation in `agenterm-ui-core`.** First pin
   both products' behaviour (thumb size, position, drag inversion, rounding) as
   tests on `ScrollbarGeometry`, then point MiniCon at it and retire
   `minicon-core/src/scrollbar.rs`.
3. **Composer editing rules** — the largest and the most likely to differ in
   intent. Survey first; decide with both owners.

## 6. How we will know it is working

- Duplicated implementations of the same rule: 3 today, trending to 0.
- Pin drift at the start of each MiniCon cycle: small, and never forced by a
  shared-crate break.
- Zero "green in AgenTerm, broken in MiniCon" events once the feature-matrix
  gate exists.
