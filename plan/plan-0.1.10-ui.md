# 0.1.10 UI — finish the ui/ui-design vision (do it properly)

0.1.8/0.1.9 shipped A+B+C+D from the `ui/ui-design/` study. 0.1.10 must finish
the two deferred pieces **completely** — no half-delivery. Both are in the design
study; this is the checklist to land them with tests and docs.

## State after 0.1.9 (done)
- **A** Simplified/Traditional Chinese switchable locale.
- **B** Per-row close button only on the active/hovered row.
- **C** Theme engine + three runtime themes (Neutral / Docs Ink / Paper Ink),
  switched today by `Ctrl+Shift+P`. The engine and all three palettes exist;
  what is missing is a *discoverable* picker (the swatch row lives in the
  settings panel = E).
- **D** Bottom status bar with a fixed-width `L###:C###` cursor readout.

## E — toolbar 7→2 + settings panel (Turn 4)
The header still shows seven tools; the design reduces it to two and moves the
rest into a settings panel. This is the headline remaining change.

1. `src/ui.rs` `Layout`: remove `help`, `language_chinese`, `language_english`,
   `zoom_out`, `zoom_reset`, `zoom_in`; keep `new_root`; add `settings`. Add a
   settings-panel rect. Update `TreeHit` (drop Help/Language/Zoom*, add
   `Settings`) and add the panel's own hit type.
2. Delete the narrow-sidebar overflow test and its doc-coupling
   (`tests/minicon_alignment.rs::the_narrow_window_limit_is_quoted_consistently`
   plus the README/PRD prose it pins) — the overflow problem is gone once the
   row holds two tools.
3. Build the settings panel by hand (no widget engine): **界面语言 / interface
   language** selector (English / 简 / 繁), **字号 / font size** (−/100%/+),
   **主题 / theme** as three clickable swatches for Neutral/Docs/Paper (the
   discoverable picker for C), and a **快捷键 / shortcuts** area that reuses
   `help_lines()` — then remove the standalone help button. Anchor the panel to
   the settings button (above/below), not a centered modal.
4. Move `Ctrl+Shift+P` theme cycling into the panel's swatches (keep the chord
   too); expose `settings_open` in the control snapshot (update the
   `minicon_blackbox` fixed-key-set test).
5. Update every header hit-test unit test in `src/ui.rs` for the new 2-tool row.

## F — grid crosshair (Turn 5)
Mouse row/column crosshair snapped to character cells, with the reading shown in
the status bar's `L###:C###` (already present from D).
1. Measure cell width at mount (a 50-char sample), snap the crosshair to
   `col × cell_w` / `row × cell_h`.
2. Draw two 1px lines at 0.28 opacity plus a 3.5%-white row/column band; both
   toggleable. Redraw only the two lines per frame (two `fill_rect`), no text
   reflow.
3. 1-based row/col to match vt100 CUP; add a unit test for the cell-snapping
   math.

## Ship it right
- Before committing any Rust: `cargo fmt && cargo clippy --all-targets -- -D
  warnings` (six-cell gates these; `cargo test` does not).
- Refresh the website screenshot afterwards with
  `scripts/capture-website-screenshot.sh` so the shot shows the 2-tool header +
  settings panel.
- Cut 0.1.10 with the dual-signed runbook `plan/plan-0.1.9-dual-signed-release.md`
  (both signing switches stay on).
