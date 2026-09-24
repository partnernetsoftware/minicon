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
│   ├── [x] A1 multi-line caret: Up/Down move inside a multi-line draft;
│   │        history recall at the first/last line, or Alt+Up/Down anywhere
│   │        invariant: a single-line draft still recalls on Up -- held
│   │        done 4825813; the rule was in the table, the branch was missing
│   ├── [x] A2 word motion: Ctrl+Left/Right, Ctrl+Backspace/Delete
│   │        done 2ca7b22; Windows rule, both ends land on word starts
│   ├── [x] A3 Home/End per line; Ctrl+Home/End for the whole draft
│   │        done 2ca7b22; Control widens, Shift selects, they compose
│   ├── [x] A4 the rules are one table: `minicon_core::keymap`, read by the
│   │        key handler and by `--help`, so the list cannot drift
│   │        done 4825813 (with A1; A1 alone would have been a fifth copy)
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

## A1 + A4, as built (2026-09-24, `4825813`)

A survey before starting changed the order. `composer::Move::Up` already moved
by line and preserved the visual column; the binary only called it under
Shift. So A1 was never an algorithm, and A2 (`word_bounds`) and A3
(`Move::LineStart`/`LineEnd`) are not either — their primitives are in the
crate too. The work is dispatch, and dispatch was spread over four copies of
one decision: a `match` in the key handler, a commit-action helper, a decline
helper, and the prose in `--help` and the README. Adding keys to that shape
would have made each new key four edits, so A4 went first.

What exists now:

- `crates/minicon-core/src/keymap.rs` — `Key`/`Modifiers`/`Chord` in,
  `Action` out. `action()` is the only reader for behaviour, `help_lines()`
  the only reader for prose. 12 unit tests.
- The clipboard modifier is a parameter (`ClipboardModifier`), not a `cfg`, so
  the macOS Command rule is tested on whatever machine runs the tests. The
  crate's no-platform boundary test forced this, and it was the right force.
- `src/main.rs` keeps only `composer_chord()` (host event → neutral chord) and
  `composer_draft_shape()` (where the caret sits). The 115-line `match` and
  the three helpers are gone.
- `--help` renders its composer lines from `HELP`; the README follows, which
  the existing alignment gate enforces.

`Draft` is the one piece of context the table takes, and it exists for exactly
one reason: Up and Down serve two jobs. The rule is "move while there is a
line to move to, recall at the edge", which keeps the single-line draft — what
is in the box nearly every time — behaving exactly as it did.

A2 and A3 are now each a row plus a `Move` variant. Do them against this
table, not against the old shape.

## A2 + A3, as built (2026-09-24, `2ca7b22`)

Each was a row in the table plus a primitive in the crate, which is what A4
was for. `Move` gained `WordLeft`/`WordRight`/`DraftStart`/`DraftEnd`;
`keymap` gained four rows; `main.rs` gained two match arms and nothing else.

Two decisions worth keeping:

- **Word motion is reversible.** Both directions land on a word's first
  character (Windows' rule), so Ctrl+Left undoes a Ctrl+Right. Landing on word
  *ends* going right would drift the caret on every round trip, and that is
  the kind of thing nobody reports and everybody feels. A test asserts the
  round trip rather than the literal offsets.
- **`word_bounds` was not reused.** It favours the token to the left so a
  double-click just past a word still selects it. Correct for selection, wrong
  for motion. Two rules that look alike are not one rule.

The alignment gate caught a real drift mid-change: the README gained
`Ctrl+Backspace` before the table did and the build went red naming the chord.
That is A4 working from the direction it was built to work from. The help
column is also measured from the widest chord now, after a hard-coded 18
overflowed on the first long one — a table that formats itself is part of
being one table.

Branch A is complete except A5, which is a non-goal and needs no work.

## B1 diagnosed (2026-09-24, runs 35978904963 / 35979574823)

**Reproduced on a hosted Windows runner, not on macOS.** The new black-box
test `zooming_all_the_way_out_keeps_the_terminal_readable` fails there with a
snapshot that says everything:

```
"child_alive": true, "font_size_px": 8, "cols": 141, "rows": 40,
"cursor": { "row": 0, "col": 0 }, "rows_text": [ "", "", "", ... ]
```

The shell is alive, the grid is sane, and every row is empty — including the
one the cursor is on. Text written *before* the zoom is gone and text typed
*after* it never arrives. So the terminal is not mis-scrolled; it has stopped
showing anything at all.

**Where it comes from** — `console_agent.rs::resize()` in `agenterm-platform`:

1. It shrinks the console window to a 1x1 rectangle (`minimal`) so the buffer
   is free to change size. That is the standard dance and is fine.
2. It sets the buffer, then sets the window to the requested size.
3. If that last `SetConsoleWindowInfo` fails, it returns `Err` — **and leaves
   the console window at 1x1**. Every later scrape then reads a one-cell
   window, which is exactly a blank screen.
4. MiniCon discards the error (`let _ = master.resize(...)`), so nothing
   anywhere reports that the console is now one cell wide.

**Why the failure needs a small font.** A console window cannot exceed
`GetLargestConsoleWindowSize`, which is set by the desktop and the console
font. The runner's desktop is 1024x768; with the usual 8x16 console font that
is about 128 columns. The failing snapshot asks for **141**. Zooming in shrinks
the request back under the limit, the call succeeds, and the terminal "heals" —
which is precisely the shape the owner reported.

`GetLargestConsoleWindowSize` appears nowhere in the crate.

**Unverified step:** that the failing call is the window resize and that the
limit is the reason. Everything up to it is observed; this last link is
inferred from the numbers. It needs one diagnostic round on Windows, not a
guess committed as a fix.

**Whose code:** `crates/agenterm-platform/src/adapters/windows/console_agent.rs`
belongs to the AgenTerm lane. Claimed by envelope; the change lands there and
MiniCon re-pins. Two things the fix owes:

- clamp the requested window to the largest the console allows, so a grid
  wider than the desktop degrades to a narrower window instead of failing;
- never return from `resize()` with the window still at `minimal` — restore
  the previous rectangle on any failure, so the worst case is a stale size
  rather than a dead screen.

MiniCon owes one thing regardless of what AgenTerm does: `apply_resize`
discards the backend's resize error. A backend that says "I could not do that"
should not be silently believed.
