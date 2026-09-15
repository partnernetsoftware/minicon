# MiniCon repo review — "温故而知新" (post-v0.1.11)

A product-thinking review of the whole repo after v0.1.11, from four parallel
surveys (code abstraction, docs freshness, stability/robustness, UI/UX). The
guiding lens: **the implementation layer is strong; the layers that make it
findable, consistent, and legible lag it.** Finish those before adding surface.

Legend: [x] done this pass · [ ] planned · effort S/M/L.

## Headline conclusions

- **Runtime is exceptionally hardened.** The stability survey found no reachable
  panics on the PTY / control / resize / paint paths (bounded wire decode,
  saturating geometry, clamped grid/font, `catch_unwind` control workers,
  disciplined teardown). Robustness work is therefore about *test robustness*
  and a few deliberately-swallowed errors, not crash bugs.
- **The 0.1.11 theming was half-applied to chrome-only widgets.** Recoloring the
  terminal body exposed that the crosshair and scrollbar were still hardcoded.
  (Fixed this pass.)
- **`main.rs` (~9k lines) blurs the chrome/terminal/control boundary.** The
  biggest structural win is lifting the chrome-paint layer into its own module;
  the deeper win is decomposing the ~50-field `ConTerminal` god-struct.
- **Doc drift lived in the top-level, non-owning docs** (`PRD.md`,
  `CODE_SIGNING_POLICY.md`) even after the owning PRD/README were refreshed.
  (Fixed this pass.)

## Done this pass

- [x] **UI theme-consistency + discoverability** (commit `686cdee`): crosshair
  follows terminal fg (was invisible white on Paper); scrollbar tones from the
  terminal palette; the L###:C### readout follows the cursor when the pointer
  is off the terminal; the settings-panel shortcut list rewritten complete +
  correct (was mislabeled `Ctrl+[ / ]`), 8→12 lines, all three languages.
- [x] **Test robustness** (commit `8408850`): `wait_for` deadlines scale by
  `MINICON_TEST_SLOWDOWN` (six-cell exports 4); the zoom test waits for the
  shell prompt before typing. Mitigation, not a full cure — see backlog.
- [x] **Doc drift** (commit `b3a0a32`): `PRD.md` frontier/subtree/mermaid and
  `CODE_SIGNING_POLICY.md` header through v0.1.11 dual-signed; README broken
  hero image + sha256 example fixed; two finished plans archived.
- [x] Earlier in the review chain: `.github/release-notes.md` template rewritten
  (root cause of stale auto-notes); website rebuilt around user value.

## Backlog — UI/UX (finish what shipped; user value / effort)

- [x] **DONE** — Composer text selection with Shift+Arrows (commit 9318db4):
  Shift+←/→ extend by character, Shift+↑/↓ extend by line, Shift+Home/End to
  line ends. The composer today has only a `select_all` boolean, so this needs
  a real anchor/caret selection model in `crates/minicon-core/src/composer.rs`
  (add `anchor: Option<usize>`, `Move::Up`/`Down` with column-preserving
  vertical motion, generalize `prepare_edit`/`cut`/`selected_text` to the
  selection range) plus **per-row highlight rendering** woven into the existing
  multi-line horizontal-window paint in `paint_host_ui` (main.rs ~3392), plus
  Shift+arrow key dispatch (main.rs ~2081). Validate with composer unit tests +
  a scripted `send-ui-keys Shift+Left` screenshot of the highlight. Do as one
  dedicated, tested unit — it's a core editing path.
- [x] Paste-review preview collapsed multiline to one line on macOS/Linux —
  FIXED (`6a5c8f3`): emit the host control's newline form (CRLF Windows, LF
  else).
- [x] Settings panel: live zoom % readout + Escape-to-close (`2878825`).

- [ ] **S** — Crosshair defaults ON with no discoverable off-switch; the
  mouse-following band is surprising for a minimal terminal. Reconsider default
  OFF, or make it clearly discoverable (now at least listed in the panel).
- [ ] **S** — Font "reset" cell shows a static "100%" that never reflects the
  real zoom after Ctrl+wheel; render the current percentage.
- [~] Font stays per-tab by design (Ctrl+wheel zooms the focused tab); the live
  zoom % readout added this pass makes the active-tab scope clear. Making the
  panel buttons global is a possible future change, not a bug.
- [x] DONE (`b3d6944`) — Tree close button now has a hover plate; a non-zero
  shell exit colors the tab title in the error color (clean exit dims to muted).
- [x] DONE (`480c11c`) — Header New/Settings buttons get a hover plate.
  (Tooltip/accessible name still open — deferred, needs a11y name plumbing.)
- [x] DONE (`b3d6944`) — Tab titles (tree + status bar) end in "…" when clipped.
- [ ] **S/M** — Settings panel: bind Escape to close and add a focus ring
  (full keyboard nav is out of scope for a minimal terminal; Escape + focus is
  the cheap high-value slice). Add hover/press feedback on panel + composer
  buttons.

## Backlog — code structure (impact / risk)

- [x] DONE — Extracted the chrome-paint cluster into `src/chrome.rs` (16 free
  fns/consts/types + their 5 unit tests): status bar, settings panel, header
  icon buttons, composer button labels, and the shared host-UI text/glyph
  layout. All are argument-driven (no `ConApp`/`ConTerminal` field access), so
  the module is readable and testable on its own. `main.rs` 9281 → 8518 lines;
  `chrome.rs` 790. `pub(crate)` surface kept minimal (internals — `PlacedGlyph`,
  `layout_text_parts`, `blit_placed_glyph`, `centered_label_origin` — stay
  private). clippy clean, 261 bin tests pass. The `center_offset`/`to_physical`/
  `host_ui_text_width` shared-helper dedupe is deferred to the `ChromeCtx` item
  below — do it there, not as a separate pass.
- [ ] **S** — Collapse the five `_checked`/unchecked PTY-write wrapper pairs into
  one `ignore_closed_pty` helper; gate the test-only `paint_cells` wrapper behind
  `#[cfg(test)]`; merge the two `impl ConTerminal` blocks.
- [ ] **S/M** — Give the `too_many_arguments` paint fns a `ChromeCtx { layout,
  theme, scale, fonts }`; drop the `#[allow]`s.
- [ ] **M** — Split `dispatch_control` (~450-line match) into per-verb handlers.
- [ ] **L** — Decompose the ~50-field `ConTerminal` god-struct into `PtyProcess`
  / `PointerGesture` / `BlinkState` / `CrosshairState`. Highest structural
  payoff, widest edit surface — do after the chrome extraction.

## Backlog — robustness / tests

- [ ] **S** — Convert the few silent `let _ =` on user paths (terminal-query
  replies in `drain_pty`, keystroke `forward_key`, clipboard/OSC-52) into
  `diagnostics::record` (and retain-and-retry the query replies).
- [ ] **S** — Clamp `hit_test` column to `cols-1` (row already is), so
  off-grid pointer/control coords don't make phantom-width selections.
- [ ] **S** — `blend_rect` should use `get`/`get_mut` like `blit_glyph` for
  index-safety symmetry.
- [ ] **M** — Zoom-test flake root: the scripted control journey can't
  adaptively wait for shell readiness, so a warmed host still races startup.
  Make the journey (or the harness) gate the producer on an observed prompt
  marker, not a fixed `wait_ms`. The deadline scaling landed; this is the cure.
- [ ] **M** — Coverage gaps: control-socket disconnect mid-op (replay-cache
  idempotency across reconnect) and PTY backpressure + clean shutdown while
  blocked; extreme font×scale metrics.

## Backlog — docs organization

- [x] DONE — Renumber the duplicate `PRD_02_28` id (qjswasm → `PRD_02_29`); the
  three cross-refs (`PRD.md`, `PRD_02_27` ×2) updated with it.
- [x] DONE — `PRD_02_29_qjswasm_horizon.md` re-anchored: dropped the stale "not
  v0.1.3 scope" (8 versions old) for "not near-term (through v0.1.12) scope"; it
  stays a genuine horizon (agenterm qjswasm+TinyVM dependency still unshipped).
- [~] DEFERRED (deliberate) — Move seven `plan/research-*.md` into `research/`
  beside their data. Rejected this pass: the seven reports cross-link each other
  and `prd/PRD_02_27` by relative path, and are referenced back from `PRD_02_27`,
  `plan-runtime-memory-next.md`, and two `research/*/README.md`. The link churn
  (~8 files) outweighs the tidiness; these are internal diagnostics, not
  user/owner docs. Revisit only if `research/` gets a proper index.
- [ ] **S** — Consolidate the "10 MiB idle RSS" story under its owner
  (`PRD_02_27`); collapse the scattered `plan/research-*` diagnostics.

## Product-thinking note

The next release (0.1.12) is best framed as a **"finish and polish" release**:
theme consistency (done), discoverability (mostly done), plus the small UI
coherence items above — no new surface, in keeping with the one-file, no-config
identity. The chrome-extraction refactor is internal quality and can land
independently of any release. `PRD_02_25_con_workspace.md` already documents the
shipped UI; as UI items above land, keep it the single source of truth.
