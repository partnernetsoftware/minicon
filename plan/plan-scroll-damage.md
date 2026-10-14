# plan-scroll-damage

The spec `{SCROLL-DAMAGE}` asks for before any code is written. It exists because
the leaf changes a **vendored crate's contract** that `src/terminal.rs` consumes,
and because writing it turned up two facts that make the leaf's stated evidence
unreachable as-is.

Read with: `plan/plan-v0.2.9.md` (the leaf), `prd/PRD_02_24_con_terminal.md` (the
owner), `src/terminal.rs` (the consumer), `third_party/vt100/src/damage.rs` (the
contract), `third_party/vt100/src/grid.rs` (the two producers).

## The reported defect

"Flashing": ordinary output makes the whole terminal area appear to re-rasterise.
The user's words were "总是感觉闪屏 … 很容易触发终端区重刷的感觉".

## What is verified

| Fact | Where |
|---|---|
| `Grid::scroll_up` and `Grid::scroll_down` call `self.mark_full()` unconditionally, so **any** scroll — which is to say any line of output at the bottom of a full screen — demands a full terminal raster | `third_party/vt100/src/grid.rs:654` (`scroll_up`), `:679` (`scroll_down`) |
| `mark_full` is the strongest damage there is: `ScreenDamage::needs_full_raster()` is `full || viewport_changed` | `third_party/vt100/src/damage.rs:137`, `:199` |
| The consumer turns that into a whole-surface invalidation | `src/terminal.rs:421` `mark_vt_damage` → `self.dirty.mark_full()`; `src/terminal.rs:391` `mark_terminal_rows` is the row-range path that never runs for a scroll |
| The parser already has a **row range** primitive with an exact oracle, used by IL/DL, and it is conservative — it never omits a row that changed | `third_party/vt100/src/damage.rs:8` `RowRange`, `:323` `assert_damage_covers`, `third_party/vt100/src/grid.rs:642` `delete_lines` marking rows rather than full |

So the row range needed for a scroll is not a new mechanism. What is missing is
the decision to use it, plus one thing the full raster was silently covering.

## The design decision

**A scroll damages the rows it moved, plus whatever else it changed.**

For `scroll_up(count)` and `scroll_down(count)` the changed band is exactly
`[scroll_top, scroll_bottom]`: rows outside the scroll region are provably
untouched (the insert at `scroll_bottom + 1` and the remove at `scroll_top`
cancel each other below the region), rows inside it all shift, and the row that
enters the region is blank. That is a `mark_rows(scroll_top, scroll_bottom + 1)`
in place of `mark_full()`.

Two things keep it honest:

1. **The viewport flag stays.** `scroll_up` already marks `viewport_changed` when
   `scrollback_offset > 0`, and the alternate screen keeps its own full-damage
   path. Both are stronger than a row range and must not be narrowed.
2. **The scrollback length is a second fact the full raster was covering.** When
   `scroll_up` pushes the removed row into the scrollback deque, the scrollbar's
   thumb changes — and the scrollbar is drawn *outside* the row band
   (`src/terminal.rs:391` clips to `terminal_right`, which excludes
   `ui::terminal_scrollbar_width`). Today `mark_full` repaints it by accident.
   Narrowing without addressing it would leave a stale thumb on every line of
   output, so the change needs a `scrollback_changed` flag on `GridDamage` /
   `ScreenDamage` — the same shape as the existing `viewport_changed` — consumed
   at `src/terminal.rs:526` `mark_scrollbar_bounds`.

Safe failure: anything the model cannot prove still escalates to full
(`mark_full_damage`, unknown escapes, the alternate screen, a resize). The change
can only ever *narrow* what is repainted, never widen it.

## The two blockers, in order

### 1. The oracle that would prove this does not run

`third_party/vt100` is a `[patch.crates-io]` path (root `Cargo.toml:167`) and is
**not a workspace member** (root `Cargo.toml:2-6`). Confirmed:

```text
$ cargo test --locked -p vt100
error: package `vt100` cannot be tested because it requires dev-dependencies
and is not a member of the workspace
```

So `third_party/vt100/src/damage.rs`'s oracle (`assert_damage_covers`, and the
tests that lean on it) — the only thing that would catch a damage range that
under-reports — is **not executed by `scripts/build.sh test`**, which runs
`cargo test --locked --workspace`. Changing the damage contract without first
making its oracle runnable would be exactly the kind of unproven claim this
repository refuses.

### 2. No host can currently present a partial frame

Even with a perfect parser, narrowing the damage changes nothing a user can see,
because the frame is upgraded to full downstream in two places:

| Host | What forces a full frame |
|---|---|
| Windows (`con`) | "Windows con redraws a complete frame meanwhile" — any non-empty, non-full candidate becomes a whole frame (`src/main.rs:3971`) |
| Unix transient host | "A transient host gives us a fresh frame" — `!host_retains_pixels` marks both the host UI and the session full on every frame (`src/main.rs:3923`) |

`{SCROLL-DAMAGE}`'s own non-goal excludes both ("changing `mark_full` callers
other than the two scroll functions"). So the leaf, as scoped, is a
**prerequisite**, not the fix: it removes the parser-level reason to repaint
everything, and the reported flashing survives until the two amplifiers are
repaired or removed. That repair is its own leaf, needs a display to prove, and
is not in 0.2.9.

## What the leaf would be, if and when it is picked up

1. Make the vendored crate's tests runnable (workspace member, or a gate script
   that tests it in isolation), and prove the oracle still passes **before**
   touching either scroll function. This is a separate, reviewable change.
2. `damage.rs`: add `scrollback_changed` to `GridDamage` and `ScreenDamage` with
   the same accessor/merge/is-empty treatment `viewport_changed` already has.
3. `grid.rs`: set it where `scroll_up` pushes into the scrollback; replace the
   two `mark_full()` calls with the region row range.
4. `src/terminal.rs`: mark the scrollbar bounds when the flag is set.
5. Extend `damage.rs`'s own tests: a scroll returns rows rather than `full`, and
   the oracle still covers every changed row including the scrollbar's cases.
6. Re-run the full local gate plus the vendored crate's suite, and state which
   cells ran.

## Evidence this leaf cannot produce on the development host

The rendered-frame half of the acceptance ("a partial repaint under a real
display") needs a host with retained pixels on a real display. The local Windows
host is neither, so that half must be produced by the Linux `xvfb-run` journey in
the six-cell CI, and the claim has to be scoped to the cells that actually ran.

## Open questions for the owner

1. **Which leaf owns the amplifiers?** The parser narrowing cannot deliver the
   reported outcome on its own. If "no flashing" is the goal, the Windows and
   transient-host full-frame decisions are the actual target and need a home with
   their own evidence (a real display, before/after frames).
2. **Does the vendored crate become a workspace member?** That changes what
   `--workspace` builds and tests for every future change, and pulls the fork's
   dev-dependencies into the lock. The alternative — a script the gate calls —
   keeps the workspace as it is and is the smaller change.
