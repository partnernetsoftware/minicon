# Design experiment: can MiniCon feed a foreign process a frame? (Q1)

> **Not in must-ship scope. This document changes no PRD capability status.**
> It exists to answer one boolean question, in this session, and then stop.
> It is a research track, not a scheduled work item; nothing here is a
> commitment to build a frame channel.

| | |
| --- | --- |
| Date | 2026-10-12 |
| Purpose | Decide whether AgenTerm can *reuse* MiniCon by presenting MiniCon's terminal **into AgenTerm's own window** (MiniCon renders pixels, the foreign process blits them, input is forwarded back). |
| Implementation location | `research/frame-channel/` (spike output); the deciding test lives beside the code it proves, in `src/main.rs` `mod tests`. |
| Prerequisite reading | `prd/PRD_02_28_shared_core.md`; `plan/plan-cross-project-reuse.md`; this repo's `AGENTS.md`. |
| Source discipline | Clean-room. Nothing here is copied from AgenTerm; the question is asked only of MiniCon's own public/internal surface. |
| Time box | **This session** — see §4. |

## §0 Background, and what is already settled

The owner is choosing between two reuse routes. Either **AgenTerm reuses
MiniCon** (the frame channel described above), or MiniCon keeps owning its own
window and the two products stay sibling binaries. The reuse route dies on a
single primitive capability that has to exist *before* any plumbing is worth
designing:

> **Q1.** Can MiniCon render one complete host-UI frame into a
> caller-provided `&mut [u32]` buffer **with no window created** — no winit
> window, no `present`?

Settled, not up for re-litigation in this experiment:

1. MiniCon already renders its terminal **cells** windowlessly for tests:
   `ConTerminal::render_to_buffer(bytes, cols, rows) -> (Vec<u32>, u32, u32)`
   (`src/terminal.rs:2513`) rasterizes a screen into a plain `Vec<u32>` with
   no window. So *terminal-content* rasterization is not the open question.
2. MiniCon's host-UI painter is written as a pure slice painter:
   `ConApp::paint_host_ui(&self, pixels: &mut [u32], width, height, candidate)`
   (`src/host_paint.rs:28`). Its signature takes no window.
3. `ConApp::new(None, None)` (`src/main.rs:957`) constructs the full app
   state — workspace, session store, composer — and existing tests already
   call it with no window present (`src/main.rs:4695`, `4709`, ...). The
   initial `ConTerminal::new` (`src/terminal.rs:241`) spawns **no** PTY
   (`master: None`, `child: None`) and no thread.
4. The windowed render entry `ConApp::render` (`src/main.rs:3389`) composes
   `session.render(window, ...)` then `paint_host_ui(...)`. It needs a
   `&PixelWindow`, but the terminal body's only window uses are
   `window.set_title(..)` and `window.request_redraw()` — side effects, not
   pixel sources.

## §1 Hard constraints (violating any voids the experiment)

- **Q1 must be answered by running MiniCon's real painter**, not a
  reimplementation of it. A hand-rolled "this is what it would draw" is not
  evidence.
- **No window may exist** during the experiment: no winit window, no
  `PixelWindow`, no `present`, no native backing. The proof is that the code
  path runs to completion without one.
- The frame must be **non-trivial**: it must contain more than one colour and
  a real share of painted pixels, and it must be dumped to a PNG as named
  evidence.
- **Prefer calling existing code over editing product source.** Any product
  edit must be minimal and behaviour-preserving, and must be reported as a
  diff.
- **Pathology detector.** *Any urge to add a fifth native primitive or to
  restructure the render path "for convenience" is a thing this experiment
  must DETECT, not satisfy; record it as a finding, do not quietly do it.*
  If a "small helper" starts turning into a subsystem, stop and write it down
  as a finding instead.
- The decision rule and the Q1 criterion are **frozen in advance** (below).
  Running the experiment must not be allowed to move the goalposts.

## §2 Minimal experiment content

| Dimension | Choice | Why this choice |
| --- | --- | --- |
| Painter under test | `ConApp::paint_host_ui` at `src/host_paint.rs:28` | This *is* the host-UI frame: sidebar, header buttons, tab rows, composer, status strip, settings. It already takes `&mut [u32]`. |
| App construction | `ConApp::new(None, None)` | Already proven windowless by existing tests; spawns no PTY. |
| Frame size | 960×600, scale from the default session (1.0) | Matches the real startup window size (`main.rs:466`), so the frame is the real shape, not a toy. |
| Damage candidate | `DirtyRegion::full()` | A first frame is a full repaint; this exercises the whole painter, not a clipped sliver. |
| Evidence | assert buffer non-trivial + write PNG to `research/frame-channel/q1-host-ui-frame.png` | A named, re-runnable artifact. |
| Supporting check | call `terminal::tests::render_to_buffer` for one screen | Shows the *terminal body* is also windowless-capable today (settled item 1), so Q1 is not silently conflating chrome with content. |

Out of scope for this experiment: the windowed `ConTerminal::render` path,
dirty-rect plumbing across processes, shared memory, and any actual blit.

## §3 Criteria table and measurement discipline

| # | Criterion | Nature | Pass condition |
| --- | --- | --- | --- |
| ① | **Q1 boolean:** a complete host-UI frame renders into a caller `&mut [u32]` with **no window** | boolean gate (deciding) | The test returns a frame; no `PixelWindow`/winit window is constructed anywhere on the path. |
| ② | Frame is non-trivial | boolean | ≥ 3 distinct 0xRRGGBB values **and** ≥ 10 % of pixels differ from the frame's top-left pixel (proves chrome, not a flat fill). |
| ③ | Miners' honest edge: if ① fails, is the fix bounded? | checklist | Minimal change ≤ ~200 changed lines of product code **and** no new subsystem ⇒ PASS-with-caveat. Otherwise ① is a KILL. |
| ④ | Named evidence exists | checklist | PNG path recorded and committed; test asserted, not eyeballed. |

Discipline: Q1 is a **boolean**; it is decided first and ②/③ are only read if
① holds. "Changed lines" excludes tests and docs. Every number is
re-runnable from the committed test.

## §4 Decision tree + kill criterion + time box

```
1. Q1 ① = does a windowless host-UI frame render? Run the test.
   - if it renders and ② holds  -> Q1 PASS. Continue to §3/③ note, then stop.
   - if it renders but ② fails  -> investigate as an implementation artefact
       (§2.6 of the experiment method): is the flat frame a property of the
       painter or of this call? Fix the call; retest. Only if it is a property
       of the painter is Q1 FAIL.
   - if it cannot be reached without a window -> record the exact call path and
       error, then apply ③: bounded fix => Q1 PASS-with-caveat; unbounded => KILL.
2. kill criterion: if ① needs an unbounded change (a new subsystem, a
   fifth native primitive, or restructuring the render path) -> Q1 is a KILL.
   Write the conclusion and stop; do not build the bounded fix to "save" it.
3. time box: this session, up to the moment ① has a decided value and its
   evidence is written. Do not design the cross-process channel here.
```

Priority reconciliation: the tree is ordered by §3's nature column — the
boolean ① is the single node that decides; ② is only a non-vacuity guard on
①; ③ is the fallback reading for a failure of ①; ④ is a bookkeeping gate on
the output. No criterion is left out of the tree.

## §5 Layout

```
plan/design-frame-channel-experiment.md   <- this spec (committed)
research/frame-channel/q1-host-ui-frame.png  <- generated evidence (committed)
src/main.rs (mod tests)                   <- the deciding `#[test]` (committed)
```

## §6 Excluded options (and why)

| Option | Why excluded |
| --- | --- |
| Build a real winit window and capture it | That is the exact thing Q1 asks us *not* to need. It answers a different question. |
| Call `ConApp::render` (the windowed entry) | Its signature demands `&PixelWindow`; reaching it would beg the question. Used only to *locate* the window dependency, not to render. |
| Re-implement a minimal painter in the spike | A reimplementation proves nothing about MiniCon's real painter. |
| Extend `ConTerminal::render` to take `Option<&PixelWindow>` now | Would be product source change to satisfy a *different* route (the terminal body); better recorded as a finding than done speculatively. |

## §7 What this experiment does NOT answer

- Whether a **live cross-process** frame channel works: shared memory region,
  dirty rects, a frame header, an update signal, latency, tearing.
- Whether AgenTerm can actually ingest and blit MiniCon's pixels.
- Whether input can be forwarded back into MiniCon's sessions.
- Whether the license/ownership boundary permits the frame channel at all.
- Whether rendering the **terminal body** needs a change: it currently takes
  `&PixelWindow` (for `set_title`/`request_redraw` only). This experiment
  proves the *host-UI* frame is windowless and notes the body as an open seam.

## §8 Conclusion (backfilled)

**Q1 = PASS.** MiniCon renders a complete host-UI frame into a caller-provided
`&mut [u32]` with **no window created**. No product source change was needed
beyond the deciding test itself.

### Evidence

- Test: `tests::a_host_ui_frame_renders_without_a_window` in `src/main.rs`
  (`+66` lines, test-only; no behaviour change to any product path).
- Command: `cargo test --bin minicon a_host_ui_frame_renders_without_a_window`
  → `test result: ok. 1 passed; 0 failed; 386 filtered out`.
- PNG: `research/frame-channel/q1-host-ui-frame.png` — valid 960×600 8-bit
  RGBA, 12475 bytes.
- Buffer contents: **81 distinct colours**; **434969 / 576000 (75.5 %)** of
  pixels differ from the frame's own top-left (sidebar) pixel. A flat fill
  would give 1 colour and 0 % — the §3② non-vacuity guard is met with margin.

### Exact call path (file:line)

```
ConApp::new(None, None)              src/main.rs:957   (no window, no PTY; master:None at src/terminal.rs:241)
  └─ ConApp::paint_host_ui(&mut [u32])  src/host_paint.rs:28  (no window parameter)
       └─ self.active_session()?           src/main.rs:1056  (session exists after new())
```

No `PixelWindow`, no winit window, and no `present` appears anywhere on this
path; the test completing at all *is* the structural proof.

### Minimal change required

**None for the host-UI frame (Q1 as scoped).** The painter was already a pure
slice painter.

### Findings for the *complete* live frame (not Q1, recorded, not built)

- **Terminal body has a window-shaped seam.** `ConTerminal::render`
  (`src/terminal.rs:2224`) takes `&PixelWindow`, but its only uses of it are
  two side effects — `window.set_title(..)` (`src/terminal.rs:2235`) and
  `window.request_redraw()` (`src/terminal.rs:2451`) — never a pixel source.
  A frame channel that carries content *and* chrome would need to lift those
  two calls out of the pixel path. This is a **bounded** change (move side
  effects to the caller), not a new subsystem — but it is a *different* route
  (the terminal body), so it is left as a finding rather than done.
- **Terminal cells are already windowless today.** `render_to_buffer`
  (`src/terminal.rs:2513`) rasterizes a screen into a plain `Vec<u32>`; the
  test calls it as a supporting check, so Q1 is not conflating chrome with
  content.
- **Pathology detector fired clean.** No urge arose to add a fifth native
  primitive or to restructure the render path; none was done.

