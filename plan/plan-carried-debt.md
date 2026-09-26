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
│      measured real idle host RSS at 21.46 MiB (debug build); see
│      `prd/PRD_02_27_con_delivery.md`'s "Runtime host memory". Still open:
│      release-build measurement and a matching six-cell lnx-aarch64 receipt.
│      Do not pick up again separately from `{UI}`.
├── D2 select cells from the change: wire the per-cell case, not just
│      documents-only. Needs its own negative-control evidence (a change that
│      should need `win-*` but doesn't touch an obviously Windows-named path)
│      before it ships — an autonomous heuristic guess is not acceptable here.
├── E1/E2 shared seam with AgenTerm                              (OWNERS)
│      ├── E1 click streak D1-D4: four behaviour divergences
│      └── E2 composer rules: survey before anything moves; A1-A3 landed in
│             MiniCon first (v0.1.24/v0.1.26), only a second consumer
│             justifies sharing them
└── F1 composer image/screenshot paste, direction decided 2026-09-26, not
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
```
