# Carried debt (from v0.1.24, still open after v0.1.26)

Not a release plan of its own — a holding pen so open leaves survive an
archived plan doc instead of being silently dropped
(`plan/archive/plan-v0.1.24.md`). Pull an item out into the next real plan
doc when it is picked up; delete its line here once it ships or is decided
`(OWNERS)`-closed.

```text
[carried] open since v0.1.18/v0.1.21, not a regression, no release blocked on them
├── C1 `capture-pane --scrollback N`: decide the semantics (cross-screen
│      stitching, viewport restore), then implement. Needs the instrument gap
│      noted in v0.1.24's B3 decision first: `max_scrollback` in the snapshot
│      reports parser capacity, not how much scrollback actually exists.
├── C3 box-drawing glyphs from cell geometry (Consolas, 1 px gap at 12 px)
│      claimed by `plan/plan-v0.2.2.md`'s `{UI}` leaf, still `BLOCKED` there —
│      needs a real Windows display host with Consolas installed (this is
│      a font-specific rendering bug, not merely "no display"; a headless
│      Xvfb container has neither Consolas nor a screen a human can look
│      at); do not pick up again separately
├── C4 idle one-tab host RSS toward 10 MiB (paused since 2026-09-06)
│      claimed by `plan/plan-v0.2.2.md`'s `{UI}` leaf. **Corrected 2026-09-26:**
│      no longer BLOCKED on "no display" — a Linux cloud session with Xvfb
│      measured real idle host RSS at 21.46 MiB (debug build), then
│      **12.01 MiB on a release build**, same container; see
│      `prd/PRD_02_27_con_delivery.md`'s "Runtime host memory". Still open:
│      a matching six-cell lnx-aarch64 receipt (this is one x86_64 cloud
│      container, not that cell). Do not pick up again separately from
│      `{UI}`.
├── D2 select cells from the change: wire the per-cell case, not just
│      documents-only. Needs its own negative-control evidence (a change that
│      should need `win-*` but doesn't touch an obviously Windows-named path)
│      before it ships — an autonomous heuristic guess is not acceptable here.
├── E1/E2 shared seam with AgenTerm                              (OWNERS)
│      ├── E1 click streak D1-D4: four behaviour divergences
│      └── E2 composer rules: survey before anything moves; A1-A3 landed in
│             MiniCon first (v0.1.24/v0.1.26), only a second consumer
│             justifies sharing them
├── F1 composer image/screenshot paste, direction decided 2026-09-26, not
       scheduled to a version. Survey and options are in
       `prd/PRD_02_25_con_workspace.md` ("Screenshot/image paste into the
       composer"). Decided shape (file-path fallback, not a structured
       attachment channel): paste-only entry (Ctrl/Cmd+V with image
       clipboard contents); save to a temp file; composer shows a thumbnail
       chip in place of path text (new non-text draft-segment type needed in
       `ComposerState`); Send substitutes the file's absolute path and goes
       through the existing PTY-write path unchanged; single image per
       submission; no per-harness support detection or warning.
       **Corrected 2026-09-26:** no platform-crate change needed after all —
       `agenterm-platform::clipboard`'s existing `available_types()`/
       `get_type()` pair already reads arbitrary clipboard types (images
       included) on all three OS adapters; the earlier "needs a new
       image-read API first" note was wrong on inspection (mirrors `{LP}`'s
       own corrected N+1 assumption in `plan/plan-v0.2.2.md`). Remaining
       work is MiniCon-owned: pick the right type name per OS, decode to a
       temp file, and add the thumbnail-chip draft segment — self-contained
       in this repo, no cross-repo dependency.
       **Progress 2026-09-26:** data-layer + paste-path landed —
       `src/clipboard_image.rs` (type-name candidates, temp-file write, unit
       tests) wired into both composer paste call sites in `src/main.rs`
       via a shared `composer_paste_text_or_image_path()` helper (text wins
       when present and non-empty; falls back to the image path only when
       the clipboard has no text). Verified end to end on Linux/X11 under
       `xvfb-run` with a real `minicon` GUI child and the real
       `agenterm-platform` clipboard adapter (not a mock) —
       `composer_paste_of_a_clipboard_image_inserts_its_temp_file_path` in
       `tests/minicon_control.rs`; macOS/Windows share the same code path
       but are unverified from this session. **Still open, not started:**
       the thumbnail-chip UI — a new non-text draft-segment type in
       `ComposerState` (`crates/minicon-core/src/composer.rs`) rendered in
       `src/host_paint.rs`, so a pasted image shows as a chip instead of a
       raw path string in the composer text. Pick this leaf up there next.
       **Visual verification 2026-09-26:** the shipped file-path fallback
       was checked with a real rendered frame, not just JSON assertions —
       `screenshot-pane` against a real `minicon` GUI under `xvfb-run` after
       seeding the X11 clipboard and sending Ctrl+V, PNG inspected directly.
       The composer correctly shows the pasted temp file's path as plain
       text; this is exactly the still-open gap the chip UI below replaces.
       **Scoping note for whoever picks up the chip UI, 2026-09-26:**
       surveyed both files enough to flag why this is a real redesign, not
       a drive-by add. `ComposerState` (`crates/minicon-core/src/composer.rs`)
       is a flat `text: String` plus a byte-offset `caret`/`anchor`; nearly
       every method (`clamped_caret`, `selection_bounds`, `insert`, `paste`,
       history recall) assumes byte-offset arithmetic into that one string,
       and `host_paint.rs`'s composer block (~line 359 on) paints it by
       slicing `self.composer.text` into a visible line window and walking
       it cell-by-cell for caret/selection geometry (`composer::cells`,
       `visible_line_window`, `line_range`). A "new non-text draft-segment
       type" has to either (a) keep `text` as the single source of truth for
       submit/PTY-write and layer a *display-only* lookup that recognizes
       this module's own `minicon-paste-<pid>-<nanos>.<ext>` temp-path
       pattern and paints a chip in place of that literal substring (no
       caret-math or selection changes, lowest risk, but the chip is not a
       real atomic edit unit — backspace/left-right still walk it character
       by character), or (b) a real mixed-segment model, which touches
       caret clamping, selection bounds, insert/paste, submit serialization
       and every host_paint.rs measurement in the same change. Option (a) is
       likely the right first cut given this widget's invariant density and
       that no host in this session has a real screen a human can review the
       result on; do not attempt (b) without that kind of review available.
└── F2 true headless render — screenshot/UI events with no window at all,
       not started, not owner-scoped, 2026-09-27. Full writeup, evidence and
       shape are owned in `prd/PRD_02_26_con_control_cli.md`'s "Transport and
       bounds" section (search `F2`) — do not duplicate it here. One-line
       summary: `--headless` today can only read/write PTY text
       (`list-tabs`/`capture-pane`/`send-text`); `screenshot-pane`,
       `send-ui-keys`/`send-mouse`/`send-wheel` and the window-creating mux
       verbs all require `attach-gui` first, which is an implementation seam
       (`dispatch_control`'s handlers take a real `&PixelWindow`), not a
       rendering one (`host_paint.rs`'s `paint_host_ui` already renders into
       a plain `&mut [u32]` buffer). Motivation: MiniCon is meant to be
       agent-driven, not just human-driven, and a fully offscreen render
       path would let an agent screenshot/drive its own UI without paying
       for a real window or even `Xvfb`.
```
