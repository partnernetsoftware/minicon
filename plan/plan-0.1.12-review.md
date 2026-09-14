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

- [ ] **M-L** — Composer text selection with Shift+Arrows (requested):
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
- [ ] **M** — Font size is per-tab while language and theme are global, in the
  same panel with no indication; make font global or label it "this tab".
- [ ] **M** — Tree close button never got the design's hover-plate / non-zero
  exit-code red dot (Turn 3); it's a plain muted "x" today.
- [ ] **S-M** — Header icon buttons (New/Settings) have no hover state or
  tooltip; add a hover plate and an accessible name.
- [ ] **S-M** — Tab titles hard-clip with no ellipsis (tree + status bar), while
  the composer already uses "…"; reuse it.
- [ ] **S/M** — Settings panel: bind Escape to close and add a focus ring
  (full keyboard nav is out of scope for a minimal terminal; Escape + focus is
  the cheap high-value slice). Add hover/press feedback on panel + composer
  buttons.

## Backlog — code structure (impact / risk)

- [ ] **S/M** — Extract the chrome-paint cluster (`main.rs:~6589-7193`:
  paint_status_bar / paint_settings_panel / paint_header_icon_button /
  host-UI text + glyph helpers) into `src/chrome.rs`; near-mechanical (no
  `ConApp` field access), with shared `center_offset`, `to_physical` (DIP), and
  `host_ui_text_width` helpers (dedupes 9× / 2× / 2× repeats).
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

- [ ] **S** — Move seven `plan/research-*.md` into `research/` beside their data.
- [ ] **S** — Consolidate the "10 MiB idle RSS" story under its owner
  (`PRD_02_27`); collapse the scattered `plan/research-*` diagnostics.
- [ ] **S** — Renumber the duplicate `PRD_02_28` id (qjswasm → `02_29`).
- [ ] **S** — `PRD_02_28_qjswasm_horizon.md` re-anchors against "not v0.1.3
  scope" (8 versions stale); re-frame or re-decide whether it's still the horizon.

## Product-thinking note

The next release (0.1.12) is best framed as a **"finish and polish" release**:
theme consistency (done), discoverability (mostly done), plus the small UI
coherence items above — no new surface, in keeping with the one-file, no-config
identity. The chrome-extraction refactor is internal quality and can land
independently of any release. `PRD_02_25_con_workspace.md` already documents the
shipped UI; as UI items above land, keep it the single source of truth.
