# v0.1.24 — the input box, and the debts that outlived three releases

Owner's brief (2026-09-23, after 0.1.23 shipped): tidy the repository, then
plan the next one. Two things earn this release: the composer is hard to edit
in, which the owner hit while using 0.1.22, and a handful of debts have been
carried since 0.1.18 without a decision. The loop itself is fast now
(`scripts/round.sh`, a hosted runner with a real desktop), so the excuse for
deferring them is gone.

## 0. Plan tree

```text
[v0.1.24] the input box becomes editable, and old debts get answered
├── A. The composer — the owner's report, and the release's spine
│   ├── [ ] A1 multi-line caret: Up/Down move inside a multi-line draft;
│   │        history recall moves to the first/last line or Alt+Up/Down
│   │        invariant: a single-line draft still recalls on Up
│   │        evidence: composer unit tests + one control-CLI journey
│   ├── [ ] A2 word motion: Ctrl+Left/Right, Ctrl+Backspace/Delete
│   ├── [ ] A3 Home/End per line; Ctrl+Home/End for the whole draft
│   ├── [ ] A4 the rules are one table, read by both the key handler and the
│   │        settings panel, so the list cannot drift from the behaviour
│   └── [ ] A5 non-goal: no modal (vim) editing. MiniCon's surface stays
│            small; revisit only if the Notepad-shaped one proves not enough
├── B. Windows gaps still open after 0.1.23
│   ├── [ ] B1 zooming out far enough blanks the terminal until the next zoom
│   │        in -- reproduces on 0.1.22, so older than the 0.1.23 work
│   ├── [ ] B2 C2 from 0.1.23: the Windows throughput receipt asserts the
│   │        ordered completion marker and the sustained rate
│   └── [ ] B3 ConPTY keeps no host scrollback; the classic path now does.
│            Decide whether ConPTY should mirror it too, or stay as is
├── C. Carried product debt (0.1.18 and 0.1.21)
│   ├── [ ] C1 `capture-pane --scrollback N`: decide the semantics
│   │        (cross-screen stitching, viewport restore), then implement
│   ├── [ ] C2 black-box test: paste and Enter arrive in two `read()`s
│   ├── [ ] C3 box-drawing glyphs from cell geometry (Consolas, 1 px at 12 px)
│   └── [ ] C4 idle one-tab host RSS toward 10 MiB (paused since 2026-09-06)
├── D. Loop work that 0.1.23 proved worth finishing
│   ├── [ ] D1 route the Windows and Linux suites through GitHub by default
│   │        in `round.sh`, court on demand -- measured 8.5 s against 20 s
│   ├── [ ] D2 select cells from the change (the detector exists; wire the
│   │        per-cell case, not just documents-only)
│   └── [ ] D3 per-stage timings in every receipt, so the baselines update
│            themselves instead of being edited by hand
└── E. Shared seam with AgenTerm
    ├── [ ] E1 click streak D1-D4: four behaviour divergences      (OWNERS)
    └── [ ] E2 composer rules: survey before anything moves. A1-A3 land in
             MiniCon first; only a second consumer justifies sharing them
```

`(OWNERS)` waits on a decision, not on work. Non-goals: no new product
surface beyond the composer's keys, no server, no modal editor, and no change
to what a Candidate means.

## 1. Where the repository stands (2026-09-23)

| tree | size | state |
| --- | --- | --- |
| `src/main.rs` | 4,572 lines | was 7,947 at 0.1.21; `ConTerminal` left in 0.1.22 |
| `src/terminal.rs` | 3,429 lines | the terminal and its tests |
| `crates/minicon-core` | composer, click, json, numeric, tree | scrollbar moved to `agenterm-ui-core` in 0.1.22 |
| `plan/` | 3 live documents | 0.1.22 and 0.1.23 stay until their leaves close |
| `prd/archive/` | v0.1.5 … v0.1.23 | one history per release |

`plan/plan-v0.1.23.md` keeps its own open leaves: the loop items this release
adopts as D, and C2/C5 which become B2/B3 here.

## 2. Rules that stay

- Cross-compile here, run it in a real environment, only then cut a Candidate.
- A missing or unready environment is BLOCKED, never a skipped pass.
- Transport bounds may be tuned; product assertions never are.
- Every speed change is measured before and after, N runs, same machine.
- A test must fail when its guard is removed.
