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
│   ├── [x] B1 zooming out far enough blanks the terminal until the next zoom
│   │        in -- reproduces on 0.1.22, so older than the 0.1.23 work
│   │        diagnosed and timed; MiniCon's half done, fix is AgenTerm's
│   ├── [x] B2 C2 from 0.1.23: the Windows throughput receipt asserts the
│   │        ordered completion marker and the sustained rate
│   │        green on both Windows cells, run 35996754365
│   └── [x] B3 ConPTY keeps no host scrollback; the classic path now does.
│            decided: no. MiniCon's scrollback is the product's, on every
│            backend; the console buffer stays a scraping detail
├── C. Carried product debt (0.1.18 and 0.1.21)
│   ├── [ ] C1 `capture-pane --scrollback N`: decide the semantics
│   │        (cross-screen stitching, viewport restore), then implement
│   ├── [x] C2 black-box test: paste and Enter arrive in two `read()`s
│   │        done f1d298b; python3 probe reads the PTY as offered, asserts
│   │        the paste and its Enter are separate chunks (unix-only)
│   ├── [ ] C3 box-drawing glyphs from cell geometry (Consolas, 1 px at 12 px)
│   └── [ ] C4 idle one-tab host RSS toward 10 MiB (paused since 2026-09-06)
├── D. Loop work that 0.1.23 proved worth finishing
│   ├── [x] D1 route the Windows and Linux suites through GitHub by default
│   │        in `round.sh`, court on demand -- measured 8.5 s against 20 s
│   │        done d991a0a (before this session; verified 2026-09-24:
│   │        `backend_of()` sends every non-osx cell to GitHub already)
│   ├── [ ] D2 select cells from the change (the detector exists; wire the
│   │        per-cell case, not just documents-only)
│   └── [x] D3 per-stage timings in every receipt, so the baselines update
│            themselves instead of being edited by hand
│            done: `scripts/render-round-baseline.py` renders
│            `plan/round-baseline.md` from the last green receipt after
│            every round; only PASS rows are a baseline
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

## B1: the theory is dead (2026-09-24, runs 35992342157 / 35993158583)

The diagnosis above named a failed `SetConsoleWindowInfo` leaving the console
window at the 1x1 rectangle. It is wrong, and the instrument that killed it is
the one this release added for exactly that purpose.

`apply_resize` now records what the backend answered. At the moment of the
blank screen, on both Windows cells:

```
"backend_resize_error": null,
"backend_resize_failures": 0,
```

The count never clears, so this is not a failure that healed. **No refusal
reached MiniCon at any point in the session** -- every `master.resize()`
returned `Ok`, and the terminal is still blank at 141x40 with an 8 px font.

Corrected 2026-09-24, after cc-agenterm found the rest of the chain: this was
written as "no resize was refused", which the counter cannot support. The
adapter's `resize()` *did* fail, and the error was dropped one layer below
MiniCon's boundary by a `let _ =` in `console_agent.rs`. A counter at a
boundary proves what crossed it, not what happened beyond it -- the same
distinction, one layer down, that the counter itself was built to make.

What survives:

- the failure is real, reproducible on both cells, and about *content* — the
  grid is sane and every row is empty;
- it is not the backend refusing a size, so clamping to
  `GetLargestConsoleWindowSize` would not have fixed it. That remains worth
  doing for its own sake, but it is not B1's cause;
- the remaining candidates are all downstream of a resize that succeeded: the
  region the agent scrapes after a buffer change, or the buffer itself being
  resized out from under the visible window.

This is what the negative control is for. A theory that fit every number we
had was refuted by the first number we did not have, and it would have been
committed as a fix in `console_agent.rs` on the strength of the fit.

`closing_the_host_takes_the_agent_and_its_child_with_it` also surfaced in
35993158583, hidden until then behind the job's `set -e`. It was the probe
exporting a shell path where PowerShell compares a native one — harness, not
product, fixed in 85c60ff.

## B1 answered (2026-09-24, run 35996044506)

The console buffer, read from outside the product while the terminal was
blank, on both Windows cells, identically:

```
"buffer": { "x": 141, "y": 540 },
"cursor": { "x": 21,  "y": 6 },
"window": { "left": 41, "right": 41, "top": 6, "bottom": 6 },
"non_blank_rows": 5, "rows_read_failed": 0,
sample: row0 "C:\a\minicon\minicon>echo ZOOM_BLANK_MARKER"
        row1 "ZOOM_BLANK_MARKER"
        row3 "C:\a\minicon\minicon>echo ZOOM_MIN_MARKER"
```

**Nothing was lost.** Every line the test wrote is still in the buffer. The
console window — `srWindow` — is a 1x1 rectangle at (41,6), and the agent
scrapes 141x40 starting from its top-left corner. That rectangle covers
columns 41.. of rows 6.., and the text lives in columns 0..21 of rows 0..6.
So the scrape reads blank cells and reports a blank screen, correctly.

The first diagnosis named this rectangle and was still wrong, which is worth
keeping straight: the *mechanism* was right, the *cause* was not. It is not a
`SetConsoleWindowInfo` failure, because `backend_resize_failures` is 0 for the
whole session — every `resize()` returned `Ok` while leaving the window 1x1.
Whether the final call succeeded without effect, was never made, or was made
with the minimal rectangle is a question inside `console_agent.rs`, which is
the AgenTerm lane's.

A note on the probe's own verdict field, which read
`content-inside-window`: it tests row membership only, and row 6 does fall
inside a one-row window. The blank is a *column* miss. A three-way label is
still much better than a number, but it has to be read against the rectangle
it is labelling.

MiniCon's side of B1 is complete: the error propagates (`ba491a4`), the
counter that refuted the first theory (`23fa6f5`), and the probe that answered
it. The fix belongs to `console_agent.rs`.

## B1 timed (2026-09-24, run 35998453182)

The watcher, once it was looking before the wheel rather than after it, caught
the transition. 954 samples, three states:

```
12:22:20.673  window {0, 0, 77, 21} = 78x22   buffer 78x522   cursor (21,3)
12:22:22.767  window {21, 3, 21, 3} = 1x1     buffer 141x540  cursor (21,3)
12:22:23.272  window {41, 6, 41, 6} = 1x1     buffer 141x540  cursor (21,6)
```

`largest` is 128x43 throughout, against a buffer 141 wide.

The window collapses in the same sample the buffer grows to 141x540, and it
collapses *onto the cursor* -- `left`/`top` equal `dwCursorPosition` exactly.
That is conhost recomputing the window after a buffer resize, not a rectangle
the adapter wrote: every rectangle it writes has `Left = 0`.

It also cannot be put back. The buffer is 141 columns and the largest window
the desktop and font allow is 128, so a window of the requested width is not
available at that font size. `resize()` still returned `Ok` and
`backend_resize_failures` is 0, so whatever happened to that call, it did not
reach MiniCon -- and not reaching MiniCon is not the same as not happening.
That part is inside `console_agent.rs`.

Three MiniCon instruments produced this, and each one killed a wrong answer:
the propagated error (`ba491a4`), the uncleared counter that refuted the
`SetConsoleWindowInfo`-failure theory (`23fa6f5`), and the vendored probe and
watcher. The first watcher round was itself useless in the same way -- its
first sample already showed the failure -- which is the same mistake one layer
up: an instrument that starts after the event it is timing.

## B3 decided: ConPTY does not get a host scrollback (2026-09-24)

**No.** The product's scrollback is MiniCon's own, on every backend, and the
classic path's console buffer is not a feature to copy.

The question read as a gap: the classic Windows path scrolls back through
shell output and ConPTY does not, so make ConPTY match. It is the wrong way
round. What the classic path has is not a second scrollback MiniCon offers --
it is the console buffer the agent has to scrape, which exists because that
backend works by reading a real console. The 500 rows of scroll room are how
the agent avoids losing lines between scrapes, not a place the user scrolls.

Two things settle it:

- **Two scrollbacks can disagree, and one of them is not ours.** Today's B1
  work is exactly that shape: the console buffer grew to 141x540 and its
  window collapsed, and nothing MiniCon did caused it or could prevent it.
  Building a user-visible feature on a buffer whose geometry another process
  recomputes means shipping that process's surprises.
- **ConPTY has no host scrollback to mirror.** A pseudoconsole has no window
  and no visible buffer; synthesising one would mean MiniCon keeping a second
  copy of what its VT parser already holds, and then keeping the two in step.

So the work B3 implies is the opposite of adding: the classic path's buffer
stays an implementation detail of scraping. If Windows scrollback feels worse
than macOS, that is a MiniCon scrollback question (`SCROLLBACK`, currently
4000 lines) and belongs with C1, which already owns scrollback semantics.

One instrument gap found on the way and left open deliberately:
`max_scrollback` in the snapshot reports the parser's capacity, not how much
scrollback exists, so nothing observable answers "how much did we keep". C1
should fix that, since it cannot decide `capture-pane --scrollback N`
semantics without it.

## B1 closed: the chain, end to end (2026-09-24)

cc-agenterm assembled the last link, and it corrects a claim made here. The
sequence, every step now evidenced:

1. Zoom out records `resize(141, 40)`.
2. `resize()` sets the window to `minimal {0,0,0,0}` to free the buffer.
3. `SetConsoleScreenBufferSize` grows the buffer to 141x540 and **succeeds** --
   the watcher timed exactly this at 12:22:22.767.
4. conhost recomputes the window and collapses it to 1x1 at the cursor.
5. `resize()` tries to set the window back to 141 columns.
   `GetLargestConsoleWindowSize` is 128x43, so the call is refused with
   `ERROR_INVALID_PARAMETER` and the window stays degenerate.
6. `resize()` returns `Err` -- and the caller drops it: `let _ = console.resize(...)`.
7. The next poll reads a 1x1 `srWindow`, scrapes one cell, and the mirror
   rebuilds around it. Blank screen; the content never moved.

**Why the counter read 0, and what that was worth.** `backend_resize_failures`
sits at MiniCon's boundary, and step 6 means nothing ever crossed it. The
reading was accurate and the conclusion drawn from it here was not: "nothing
reached us" was written up as "nothing happened". Both lanes made the same
mistake in opposite directions on the same day -- theirs was accepting the
refutation and discarding a correct code reading, ours was overstating what a
boundary counter can see. Two true observations were treated as exclusive when
they were describing different layers.

What the instruments were actually worth: the counter did not find the cause,
but it destroyed a theory that fit every number available and would otherwise
have been committed as a fix. The dump and the watcher then supplied the
timing that no amount of reading the code could settle.

The fix is two changes in `console_agent.rs`, both AgenTerm's: clamp the
window to `GetLargestConsoleWindowSize` (still zero hits in that crate), and
stop dropping the error -- restoring the previous rectangle on failure, so the
worst case is a stale size rather than a dead screen. Their negative control
for the second one runs through MiniCon: with the error propagated and the
clamp removed, `backend_resize_failures` must go from 0 to non-zero, which
verifies the counter and the explanation for its earlier reading at once.

## Throughput on hosted Linux sits on the floor (runs 36009129332, 36012496339)

The journey pushes 33,439,744 bytes through one tab and requires 2 MiB/s.
Rates, now printed on every outcome (`THROUGHPUT:` line, probe runs the
suite with `--nocapture`):

| cell | rate (B/s) | note |
|---|---|---|
| osx-aarch64 (the Mac, local) | 4,675,943 | real GPU, real display |
| lnx-x86_64 (hosted, Xvfb) | 2,577,791 | 1.23x the floor |
| lnx-aarch64 (hosted, Xvfb) | 2,167,006 / 1,969,204 | one pass, one fail, 6% either side of the floor |
| win-x86_64 / win-aarch64 | 98,571,364 / 59,717,155 | not a drain rate: the console agent scrapes the screen, the marker shows the moment the console shows it |

The generator (`yes | head -c`) is not the bottleneck; it is faster than
any of these by two orders of magnitude. What the Linux number measures is
MiniCon draining and parsing 32 MiB under software rendering on a shared
runner, and on the arm64 runner that lands within measurement noise of the
floor. The floor stays: it is the product's promise on a real machine, and
the Mac meets it with 2.2x to spare. What this leaves is an honest gap:
the hosted arm64 Linux cell will flap on this one test until the Linux
drain path is profiled (where do 13 s go on x86_64 when the Mac needs 7?).
That profiling is product work for a later branch, not a test edit. A
failure on that cell that prints a rate within 10% of the floor is this
gap, not a regression; a rate well below it is.

Round integrity, same session: `gh run list` failing behind the proxy gave
an empty run id, and the round polled GitHub about run "" for the whole
40-minute window. An empty id now records BLOCKED for every routed cell
at once, keeping the bundle tag so the dispatched run can still be read.

## D2 stays open on purpose

`select_cells()` already answers "does anything compiled change" (all cells
or none). The finer version -- which cells a specific change actually needs
-- means trusting a path pattern to predict which target a change affects,
and a wrong guess there is a silent skip: the exact failure mode this
document's own rules forbid ("never fake a green gate"). That heuristic
needs its own negative-control evidence (a change that should need `win-*`
but doesn't touch an obviously Windows-named path) before it ships, not an
autonomous guess. Left for a session that can run that control.
