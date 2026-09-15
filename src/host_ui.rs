//! Host-UI painting: everything drawn around the terminal grid.
//!
//! These are the status bar, the settings panel, the header icon buttons, the
//! composer button labels, and the shared host-UI text/glyph layout the rest of
//! the host UI is built from. They are deliberately free functions that take
//! their geometry, theme, scale and fonts as arguments and touch no `ConApp` or
//! `ConTerminal` state, so the host-UI layer can be read, tested and changed
//! without the ~9k-line render/state file around it. The terminal *grid* is
//! painted in `terminal_paint`; this module never touches cell contents.

use crate::DEFAULT_FONT_PX;
use crate::palette::Rgb;
use crate::raster_surface::{CellRect, Surface};
use crate::{font, theme, ui};
use minicon_core::composer;

/// Paints one host UI button's label centred in its box.
///
/// Centring is computed rather than tuned: the composer's two buttons differ
/// in label width and each is half the height of the single control they
/// replaced, so any fixed inset that suits one misplaces the other. A label
/// wider than its box is left-aligned and clipped by the painter, which keeps
/// the first characters readable instead of centring the middle of a word.
pub(crate) const BUTTON_LABEL_SIZE_PX: u16 = 15;
pub(crate) const BUTTON_HINT_SIZE_PX: u16 = 11;

pub(crate) fn scaled_host_ui_font(nominal: u16, logical_font_size: f64, display_scale: f64) -> u16 {
    let display_scale = display_scale.clamp(1.0, 4.0);
    minicon_core::numeric::round_f64(
        f64::from(nominal) * logical_font_size / DEFAULT_FONT_PX * display_scale,
    )
    // Layout dimensions are already expressed as DIPs multiplied by the
    // display scale. Apply the same rule to glyphs: omitting it made host UI
    // text half-sized beside terminal text on a Retina display.
    .clamp(7.0 * display_scale, 20.0 * display_scale) as u16
}

pub(crate) fn paint_button_label(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    label: &str,
    color: Rgb,
    font_size_px: u16,
) {
    let metrics = font::cell_metrics(font_size_px);
    let label_width = metrics
        .width
        .max(1)
        .saturating_mul(u32::try_from(composer::cells(label)).unwrap_or(u32::MAX));
    let (x, y) = centered_label_origin(button, label_width, metrics.height.max(1));
    paint_host_ui_text(surface, x, y, label, color, font_size_px, button.width);
}

/// Where a label of `label_width` by `label_height` sits inside `button`.
///
/// Centring is computed rather than tuned, and it must stay inside the button
/// for every label: a label wider or taller than its box has no slack to
/// centre in, so the saturating subtraction pins it to the button's origin
/// instead of letting it start before the button. The painter then clips it,
/// keeping the label's first characters readable rather than centring the
/// middle of a word.
fn centered_label_origin(button: ui::Rect, label_width: u32, label_height: u32) -> (u32, u32) {
    let x = button
        .x
        .saturating_add(button.width.saturating_sub(label_width) / 2);
    let y = button
        .y
        .saturating_add(button.height.saturating_sub(label_height) / 2);
    (x, y)
}

pub(crate) fn paint_two_line_button_label(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    label: &str,
    hint: &str,
    color: Rgb,
    label_font_size_px: u16,
    hint_font_size_px: u16,
) {
    let label_height = font::cell_metrics(label_font_size_px).height.max(1);
    let hint_height = font::cell_metrics(hint_font_size_px).height.max(1);
    let gap = u32::from(hint_font_size_px >= 14).saturating_add(1);
    let block_height = label_height.saturating_add(gap).saturating_add(hint_height);
    let block_y = button
        .y
        .saturating_add(button.height.saturating_sub(block_height) / 2);

    let paint_line = |surface: &mut Surface<'_>, text: &str, y: u32, font_size_px: u16| {
        let metrics = font::cell_metrics(font_size_px);
        let text_width = metrics
            .width
            .max(1)
            .saturating_mul(u32::try_from(composer::cells(text)).unwrap_or(u32::MAX));
        let x = button
            .x
            .saturating_add(button.width.saturating_sub(text_width) / 2);
        paint_host_ui_text(
            surface,
            x,
            y,
            text,
            color,
            font_size_px,
            button.x.saturating_add(button.width).saturating_sub(x),
        );
    };

    paint_line(surface, label, block_y, label_font_size_px);
    paint_line(
        surface,
        hint,
        block_y.saturating_add(label_height).saturating_add(gap),
        hint_font_size_px,
    );
}

/// Paints the bottom status bar: an optional left label (the active tab) and a
/// fixed-width `L###:C###` cursor readout pinned to the right. The readout keeps
/// a constant width (zero-padded, three digits) so the bar never reflows as the
/// cursor moves — the same no-jitter rule the crosshair will rely on. Rows and
/// columns are 1-based to match vt100 CUP coordinates.
pub(crate) fn paint_status_bar(
    surface: &mut Surface<'_>,
    layout: ui::Layout,
    theme: theme::Theme,
    left_label: &str,
    cursor: Option<(u16, u16)>,
    font_size_px: u16,
    scale: f64,
) {
    let bar = layout.status;
    if bar.width == 0 || bar.height == 0 {
        return;
    }
    let dip = |value: f64| minicon_core::numeric::round_f64(value * scale.max(1.0)).max(0.0) as u32;
    surface.fill_rect(
        bar.x,
        bar.y,
        bar.width,
        bar.height,
        theme.sidebar_bg.to_xrgb(),
    );
    // A one-pixel rule separates the bar from the composer above it.
    surface.fill_rect(bar.x, bar.y, bar.width, 1, theme.border.to_xrgb());
    let metrics = font::cell_metrics(font_size_px);
    let cell_w = metrics.width.max(1);
    let pad = dip(12.0);
    let text_y = bar
        .y
        .saturating_add(bar.height.saturating_sub(metrics.height) / 2);
    // Right: the fixed-width cursor readout.
    let clamp3 = |v: u32| v.min(999);
    let readout = match cursor {
        Some((row, col)) => format!(
            "L{:03}:C{:03}",
            clamp3(u32::from(row).saturating_add(1)),
            clamp3(u32::from(col).saturating_add(1))
        ),
        None => "L---:C---".to_owned(),
    };
    let readout_width = cell_w.saturating_mul(readout.chars().count() as u32);
    let readout_x = bar
        .x
        .saturating_add(bar.width)
        .saturating_sub(pad)
        .saturating_sub(readout_width)
        .max(bar.x);
    paint_host_ui_text(
        surface,
        readout_x,
        text_y,
        &readout,
        theme.muted,
        font_size_px,
        readout_width,
    );
    // Left: the active tab label, clipped so it never runs into the readout.
    if !left_label.is_empty() {
        let left_x = bar.x.saturating_add(pad);
        let left_max = readout_x.saturating_sub(dip(8.0)).saturating_sub(left_x);
        paint_host_ui_text_parts_clipped(
            surface,
            left_x,
            text_y,
            &[left_label],
            theme.muted,
            font_size_px,
            left_max,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_settings_panel(
    surface: &mut Surface<'_>,
    layout: ui::Layout,
    scale: f64,
    theme: theme::Theme,
    ui_language: ui::UiLanguage,
    ui_theme: theme::ThemeChoice,
    shortcuts: [&str; 12],
    font_size_px: u16,
    font_percent: u16,
) {
    let dip = |value: f64| minicon_core::numeric::round_f64(value * scale.max(1.0)).max(0.0) as u32;
    let s = layout.settings_panel(scale);
    let strings = ui_language.strings();
    let metrics = font::cell_metrics(font_size_px);
    surface.fill_rect(
        s.panel.x,
        s.panel.y,
        s.panel.width,
        s.panel.height,
        theme.panel_bg.to_xrgb(),
    );
    stroke_rect(surface, s.panel, dip(1.0).max(1), theme.border);
    let inner_x = s.panel.x.saturating_add(dip(12.0));
    let inner_w = s.panel.width.saturating_sub(dip(24.0));
    let text_dy = |rect: ui::Rect| {
        rect.y
            .saturating_add(rect.height.saturating_sub(metrics.height) / 2)
    };
    let section_label = |surface: &mut Surface<'_>, first: ui::Rect, label: &str| {
        paint_host_ui_text(
            surface,
            inner_x,
            first
                .y
                .saturating_sub(metrics.height)
                .saturating_sub(dip(2.0)),
            label,
            theme.muted,
            font_size_px,
            inner_w,
        );
    };
    let option = |surface: &mut Surface<'_>, rect: ui::Rect, label: &str, active: bool| {
        let bg = if active { theme.accent } else { theme.surface };
        let fg = if active { theme.panel_bg } else { theme.text };
        surface.fill_rect(rect.x, rect.y, rect.width, rect.height, bg.to_xrgb());
        paint_host_ui_text(
            surface,
            rect.x.saturating_add(dip(8.0)),
            text_dy(rect),
            label,
            fg,
            font_size_px,
            rect.width,
        );
    };
    // Interface language
    section_label(surface, s.language[0], strings.settings_language);
    let langs = [
        ui::UiLanguage::English,
        ui::UiLanguage::ChineseSimplified,
        ui::UiLanguage::ChineseTraditional,
    ];
    for (rect, lang) in s.language.iter().zip(langs) {
        option(surface, *rect, lang.entry_label(), ui_language == lang);
    }
    // Font size
    section_label(surface, s.font_down, strings.settings_font);
    option(surface, s.font_down, "-", false);
    // The reset cell shows the current zoom, so it reads as a live readout
    // rather than a misleading static "100%" after Ctrl+wheel zooming.
    let font_percent_label = format!("{font_percent}%");
    option(surface, s.font_reset, &font_percent_label, false);
    option(surface, s.font_up, "+", false);
    // Theme (each swatch shows that theme's own canvas + accent stripe)
    section_label(surface, s.theme[0], strings.settings_theme);
    let choices = [
        theme::ThemeChoice::Neutral,
        theme::ThemeChoice::Docs,
        theme::ThemeChoice::Paper,
    ];
    for (rect, choice) in s.theme.iter().zip(choices) {
        let pal = theme::Theme::for_choice(choice);
        surface.fill_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            pal.canvas_bg.to_xrgb(),
        );
        // an accent stripe so dark and light swatches are both legible
        surface.fill_rect(
            rect.x,
            rect.y.saturating_add(rect.height.saturating_sub(dip(5.0))),
            rect.width,
            dip(5.0),
            pal.accent.to_xrgb(),
        );
        let border = if ui_theme == choice {
            theme.accent
        } else {
            theme.border
        };
        let stroke = if ui_theme == choice {
            dip(2.0).max(2)
        } else {
            dip(1.0).max(1)
        };
        stroke_rect(surface, *rect, stroke, border);
    }
    // Shortcuts (the reused help text)
    section_label(
        surface,
        ui::Rect {
            x: inner_x,
            y: s.shortcuts_y,
            width: inner_w,
            height: metrics.height,
        },
        strings.settings_shortcuts,
    );
    let mut y = s.shortcuts_y;
    for (index, line) in shortcuts.into_iter().enumerate() {
        paint_host_ui_text(
            surface,
            inner_x,
            y,
            line,
            if index == 0 { theme.text } else { theme.muted },
            font_size_px,
            inner_w,
        );
        y = y.saturating_add(s.shortcut_line_height);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeaderIcon {
    NewRoot,
    Settings,
}

pub(crate) fn stroke_rect(surface: &mut Surface<'_>, rect: ui::Rect, stroke: u32, color: Rgb) {
    let stroke = stroke.max(1).min(rect.width).min(rect.height);
    surface.fill_rect(rect.x, rect.y, rect.width, stroke, color.to_xrgb());
    surface.fill_rect(
        rect.x,
        rect.y.saturating_add(rect.height.saturating_sub(stroke)),
        rect.width,
        stroke,
        color.to_xrgb(),
    );
    surface.fill_rect(rect.x, rect.y, stroke, rect.height, color.to_xrgb());
    surface.fill_rect(
        rect.x.saturating_add(rect.width.saturating_sub(stroke)),
        rect.y,
        stroke,
        rect.height,
        color.to_xrgb(),
    );
}

/// Paints the header as one aligned, borderless icon family.
///
/// The full 24-DIP rectangles remain the hit targets, but no permanent box is
/// drawn around them. Selection uses a quiet background and short underline;
/// the icon itself stays high-contrast. Geometry avoids depending on a symbol
/// font while keeping the zoom family to the simplest possible marks.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_header_icon_button(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    icon: HeaderIcon,
    color: Rgb,
    selected: bool,
    selected_bg: Rgb,
    _font_size_px: u16,
    scale: f64,
) {
    let stroke = minicon_core::numeric::round_f64(scale.clamp(1.0, 4.0)).clamp(1.0, 4.0) as u32;
    let inset = stroke.saturating_mul(3);
    if selected {
        surface.fill_rect(
            button.x,
            button.y,
            button.width,
            button.height,
            selected_bg.to_xrgb(),
        );
        let indicator_width = button.width / 2;
        surface.fill_rect(
            button
                .x
                .saturating_add(button.width.saturating_sub(indicator_width) / 2),
            button
                .y
                .saturating_add(button.height.saturating_sub(stroke)),
            indicator_width,
            stroke,
            color.to_xrgb(),
        );
    }

    match icon {
        HeaderIcon::NewRoot => {
            let window = ui::Rect {
                x: button.x.saturating_add(inset),
                y: button.y.saturating_add(inset),
                width: button.width.saturating_sub(inset.saturating_mul(2)),
                height: button.height.saturating_sub(inset.saturating_mul(2)),
            };
            // An open terminal tray avoids reading as another button border.
            surface.fill_rect(
                window.x,
                window.y.saturating_add(stroke.saturating_mul(2)),
                stroke,
                window.height.saturating_sub(stroke.saturating_mul(2)),
                color.to_xrgb(),
            );
            surface.fill_rect(
                window.x,
                window
                    .y
                    .saturating_add(window.height.saturating_sub(stroke)),
                window.width,
                stroke,
                color.to_xrgb(),
            );
            let plus_x = button.x.saturating_add(
                button
                    .width
                    .saturating_sub(inset + stroke.saturating_mul(2)),
            );
            let plus_y = button.y.saturating_add(
                button
                    .height
                    .saturating_sub(inset + stroke.saturating_mul(2)),
            );
            surface.fill_rect(
                plus_x.saturating_sub(stroke.saturating_mul(2)),
                plus_y,
                stroke.saturating_mul(5),
                stroke,
                color.to_xrgb(),
            );
            surface.fill_rect(
                plus_x,
                plus_y.saturating_sub(stroke.saturating_mul(2)),
                stroke,
                stroke.saturating_mul(5),
                color.to_xrgb(),
            );
        }
        HeaderIcon::Settings => {
            // A sliders/adjust glyph: three tracks, each with an offset knob.
            let left = button.x.saturating_add(inset);
            let track_w = button.width.saturating_sub(inset.saturating_mul(2));
            let gap = button
                .height
                .saturating_sub(inset.saturating_mul(2))
                .checked_div(3)
                .unwrap_or(stroke);
            let knob = stroke.saturating_mul(2);
            let offsets = [track_w / 5, track_w * 3 / 5, track_w * 2 / 5];
            for (i, off) in offsets.iter().enumerate() {
                let y = button
                    .y
                    .saturating_add(inset)
                    .saturating_add(gap.saturating_mul(i as u32))
                    .saturating_add(gap / 2);
                surface.fill_rect(left, y, track_w, stroke, color.to_xrgb());
                surface.fill_rect(
                    left.saturating_add(*off),
                    y.saturating_sub(knob),
                    knob,
                    knob.saturating_mul(2).saturating_add(stroke),
                    color.to_xrgb(),
                );
            }
        }
    }
}

pub(crate) fn paint_host_ui_text(
    surface: &mut Surface<'_>,
    x: u32,
    y: u32,
    text: &str,
    color: Rgb,
    font_size_px: u16,
    max_width: u32,
) {
    paint_host_ui_text_parts(surface, x, y, &[text], color, font_size_px, max_width);
}

/// Like [`paint_host_ui_text_parts`], but when the text does not fit it drops
/// trailing glyphs and paints a single-cell "…" so a clipped tab title reads as
/// truncated rather than cut mid-glyph. Used where a name can outrun its column
/// (the tab tree, the status bar).
pub(crate) fn paint_host_ui_text_parts_clipped(
    surface: &mut Surface<'_>,
    x: u32,
    y: u32,
    parts: &[&str],
    color: Rgb,
    font_size_px: u16,
    max_width: u32,
) {
    let metrics = font::cell_metrics(font_size_px);
    let cell_w = metrics.width.max(1);
    let cell_h = metrics.height.max(1);
    let limit = x.saturating_add(max_width).min(surface.width);
    let total: usize = parts.iter().map(|part| part.chars().count()).sum();
    let placed = layout_text_parts(parts, x, cell_w, limit);
    let ellipsis_x = if placed.len() >= total {
        None
    } else {
        // Re-lay out with one cell reserved for the marker, then place it after
        // the last glyph that fit.
        let reduced = layout_text_parts(parts, x, cell_w, limit.saturating_sub(cell_w));
        let ellipsis_x = reduced.last().map_or(x, |glyph| glyph.x + glyph.width);
        for placed in &reduced {
            blit_placed_glyph(surface, placed, y, cell_h, color, font_size_px);
        }
        Some(ellipsis_x)
    };
    if let Some(ellipsis_x) = ellipsis_x {
        blit_placed_glyph(
            surface,
            &PlacedGlyph {
                character: '…',
                x: ellipsis_x,
                width: cell_w,
            },
            y,
            cell_h,
            color,
            font_size_px,
        );
        return;
    }
    for placed in &placed {
        blit_placed_glyph(surface, placed, y, cell_h, color, font_size_px);
    }
}

fn blit_placed_glyph(
    surface: &mut Surface<'_>,
    placed: &PlacedGlyph,
    y: u32,
    cell_h: u32,
    color: Rgb,
    font_size_px: u16,
) {
    if surface.intersects_rect(placed.x, y, placed.width, cell_h)
        && let Some(glyph) = font::raster(placed.character, font_size_px)
    {
        surface.blit_glyph(
            &glyph,
            CellRect {
                x: placed.x,
                y,
                w: placed.width,
                h: cell_h,
            },
            color,
            0.0,
        );
    }
}

pub(crate) fn paint_host_ui_text_parts(
    surface: &mut Surface<'_>,
    x: u32,
    y: u32,
    parts: &[&str],
    color: Rgb,
    font_size_px: u16,
    max_width: u32,
) {
    let metrics = font::cell_metrics(font_size_px);
    let cell_w = metrics.width.max(1);
    let cell_h = metrics.height.max(1);
    let limit = x.saturating_add(max_width).min(surface.width);
    for placed in layout_text_parts(parts, x, cell_w, limit) {
        if surface.intersects_rect(placed.x, y, placed.width, cell_h)
            && let Some(glyph) = font::raster(placed.character, font_size_px)
        {
            surface.blit_glyph(
                &glyph,
                CellRect {
                    x: placed.x,
                    y,
                    w: placed.width,
                    h: cell_h,
                },
                color,
                0.0,
            );
        }
    }
}

/// One character placed on the grid: where it starts and how many pixels wide.
struct PlacedGlyph {
    character: char,
    x: u32,
    width: u32,
}

/// Lays `parts` out left to right from `x`, stopping at the first character
/// that would cross `limit`.
///
/// Separate from the painter so the two rules it encodes are testable without a
/// font: the parts are **concatenated** with nothing between them (a segment
/// boundary is not a space), and each character advances by its own cell count
/// — a double-width character owns two cells, a soft break owns none — so the
/// clip lands between characters rather than through a wide glyph. A character
/// the limit cannot hold ends the layout, so nothing after it is placed either.
fn layout_text_parts(parts: &[&str], x: u32, cell_w: u32, limit: u32) -> Vec<PlacedGlyph> {
    let cell_w = cell_w.max(1);
    let mut placed = Vec::new();
    let mut cursor = x;
    for part in parts {
        for character in part.chars() {
            let span_w = cell_w.saturating_mul(composer::character_cells(character) as u32);
            if cursor.saturating_add(span_w) > limit {
                return placed;
            }
            placed.push(PlacedGlyph {
                character,
                x: cursor,
                width: span_w,
            });
            cursor = cursor.saturating_add(span_w);
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_ui_font_tracks_terminal_zoom_and_display_scale() {
        assert_eq!(scaled_host_ui_font(15, DEFAULT_FONT_PX, 1.0), 15);
        assert_eq!(scaled_host_ui_font(15, DEFAULT_FONT_PX, 2.0), 30);
        assert!(scaled_host_ui_font(15, 8.0, 1.0) < 15);
        assert!(scaled_host_ui_font(15, 24.0, 1.0) > 15);
        assert_eq!(scaled_host_ui_font(15, 36.0, 1.0), 20);
        assert_eq!(scaled_host_ui_font(15, 36.0, 2.0), 40);
    }

    /// A label narrower than its button is centred with the leftover split two
    /// ways; one wider or taller than the button has no slack, so it pins to
    /// the button's origin (left-aligned) rather than starting before it. The
    /// origin must never leave the button.
    #[test]
    fn a_label_is_centred_when_it_fits_and_pinned_when_it_does_not() {
        let button = ui::Rect {
            x: 40,
            y: 12,
            width: 100,
            height: 24,
        };
        // Narrower: centred. Leftover 40 splits to 20 each side.
        assert_eq!(centered_label_origin(button, 60, 10), (40 + 20, 12 + 7));
        // Exactly the button's size: no slack, so the origin is the button's.
        assert_eq!(centered_label_origin(button, 100, 24), (40, 12));
        // Wider and taller: still the button's origin, never before it.
        assert_eq!(centered_label_origin(button, 500, 500), (40, 12));
        // One pixel of slack rounds down, keeping the extra cell on the right.
        assert_eq!(centered_label_origin(button, 99, 23), (40, 12));
        assert_eq!(centered_label_origin(button, 98, 22), (40 + 1, 12 + 1));
    }

    /// The host-UI text layout: parts are concatenated with nothing inserted
    /// between them, each character advances by its own cell count, and the
    /// first character the limit cannot hold ends the layout so nothing after
    /// it is placed. These are the rules the painter used to hold inline.
    #[test]
    fn host_ui_text_layout_concatenates_and_clips_between_characters() {
        let placed = |parts: &[&str], x: u32, cell: u32, limit: u32| {
            layout_text_parts(parts, x, cell, limit)
                .into_iter()
                .map(|g| (g.character, g.x, g.width))
                .collect::<Vec<_>>()
        };

        // Two parts are adjacent: "ab" + "cd" lays out as abcd with no gap.
        assert_eq!(
            placed(&["ab", "cd"], 0, 8, 32),
            vec![('a', 0, 8), ('b', 8, 8), ('c', 16, 8), ('d', 24, 8)]
        );
        // An empty part contributes nothing and does not advance the cursor.
        assert_eq!(
            placed(&["ab", "", "cd"], 0, 8, 32),
            vec![('a', 0, 8), ('b', 8, 8), ('c', 16, 8), ('d', 24, 8)]
        );
        // A leading empty part leaves the first character at x.
        assert_eq!(placed(&["", "z"], 100, 8, 200), vec![('z', 100, 8)]);

        // A double-width character owns two cells, and the clip falls between
        // characters: 3 narrow (24 px) + one wide (16 px) needs a 40 px limit.
        assert_eq!(
            placed(&["abc\u{4e2d}"], 0, 8, 40),
            vec![('a', 0, 8), ('b', 8, 8), ('c', 16, 8), ('\u{4e2d}', 24, 16)]
        );
        // One pixel less and the wide glyph cannot fit, so it and everything
        // after it are dropped — the clip never cuts a glyph in half.
        assert_eq!(
            placed(&["abc\u{4e2d}"], 0, 8, 39),
            vec![('a', 0, 8), ('b', 8, 8), ('c', 16, 8)]
        );
        // The limit is exclusive: a character ending exactly at it is kept.
        assert_eq!(placed(&["ab"], 0, 8, 16).len(), 2);
        assert_eq!(placed(&["abc"], 0, 8, 16).len(), 2);

        // A soft break occupies no cells, so it is placed at the cursor without
        // moving it and the text after it lands at the same x.
        let with_break = placed(&["a\nb"], 0, 8, 64);
        assert_eq!(with_break, vec![('a', 0, 8), ('\n', 8, 0), ('b', 8, 8)]);

        // An empty layout is empty, not a zero-width placeholder.
        assert!(placed(&[], 0, 8, 64).is_empty());
        assert!(placed(&[""], 0, 8, 64).is_empty());
    }

    #[test]
    fn host_ui_text_parts_match_joined_text_with_clipping() {
        let width = 160;
        let height = 32;
        let mut joined_pixels = vec![0; width * height];
        let mut parts_pixels = vec![0; width * height];
        paint_host_ui_text(
            &mut Surface::new(&mut joined_pixels, width as u32, height as u32),
            3,
            2,
            "@12  中abc|",
            Rgb(240, 240, 240),
            14,
            73,
        );
        paint_host_ui_text_parts(
            &mut Surface::new(&mut parts_pixels, width as u32, height as u32),
            3,
            2,
            &["@", "12", "  ", "中abc", "|"],
            Rgb(240, 240, 240),
            14,
            73,
        );
        assert_eq!(parts_pixels, joined_pixels);
    }

    /// A double-width character must consume two cells in host UI text exactly
    /// as it does in the terminal grid. Asserted by composition rather than by
    /// glyph appearance: painting "中A" in one call must equal painting "中"
    /// then "A" two cells along. Under the one-cell-per-character advance this
    /// replaced, the "A" landed one narrow cell in -- on top of the truncated
    /// right half of the wide glyph -- which is what made CJK typed into the
    /// composer render as overlapping garbage.
    #[test]
    fn wide_host_ui_characters_occupy_two_cells() {
        let width = 160u32;
        let height = 32u32;
        let size = 15u16;
        let cell_w = font::cell_metrics(size).width.max(1);
        let color = Rgb(240, 240, 240);

        let mut together = vec![0u32; (width * height) as usize];
        paint_host_ui_text(
            &mut Surface::new(&mut together, width, height),
            3,
            2,
            "中A",
            color,
            size,
            width,
        );

        let mut apart = vec![0u32; (width * height) as usize];
        {
            let mut surface = Surface::new(&mut apart, width, height);
            paint_host_ui_text(&mut surface, 3, 2, "中", color, size, width);
            paint_host_ui_text(&mut surface, 3 + 2 * cell_w, 2, "A", color, size, width);
        }

        assert_eq!(
            together, apart,
            "a wide character must advance two cells, leaving the next glyph clear of it"
        );
    }
}
