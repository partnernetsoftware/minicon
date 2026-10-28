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

/// Disabled navigation remains visible without competing with usable actions.
pub(crate) fn browser_navigation_color(theme: theme::Theme, enabled: bool) -> Rgb {
    if enabled {
        return theme.text;
    }
    let mix = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
    Rgb(
        mix(theme.muted.0, theme.sidebar_bg.0),
        mix(theme.muted.1, theme.sidebar_bg.1),
        mix(theme.muted.2, theme.sidebar_bg.2),
    )
}

/// Paints an independent one-line host editor with the same selection and
/// preedit metrics as the address field. Native page focus never owns it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_single_line_host_editor(
    surface: &mut Surface<'_>,
    rect: ui::Rect,
    editor: &composer::ComposerState,
    focused: bool,
    placeholder: &str,
    theme: theme::Theme,
    font_size: u16,
    scale: f64,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    surface.fill_rect(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        theme.canvas_bg.to_xrgb(),
    );
    let border = if editor.submit_error.is_some() {
        theme.error
    } else if focused {
        theme.accent
    } else {
        theme.border
    };
    stroke_rect(surface, rect, scale.max(1.0) as u32, border);
    let pad = ((8.0 * scale) as u32).min(rect.width / 2);
    let x = rect.x.saturating_add(pad);
    let metrics = font::cell_metrics(font_size);
    if rect.height < metrics.height {
        return;
    }
    let y = rect
        .y
        .saturating_add(rect.height.saturating_sub(metrics.height) / 2);
    let width = rect.width.saturating_sub(pad.saturating_mul(2));
    let cell = metrics.width.max(1);
    if !focused {
        paint_host_ui_text(
            surface,
            x,
            y,
            if editor.text.is_empty() {
                placeholder
            } else {
                &editor.text
            },
            if editor.text.is_empty() {
                theme.muted
            } else {
                theme.text
            },
            font_size,
            width,
        );
        return;
    }
    let view = composer::visible_window(
        &editor.text,
        &editor.preedit,
        editor.caret,
        1,
        (width / cell).max(1) as usize,
    );
    if editor.preedit.is_empty()
        && let Some((start, end)) = composer::selection_bounds(editor)
    {
        let start = start.max(view.text);
        let end = end.max(view.text);
        let offset = (composer::cells(&editor.text[view.text..start]) as u32 * cell).min(width);
        let selected_width = (composer::cells(&editor.text[start..end]) as u32 * cell)
            .min(width.saturating_sub(offset));
        surface.fill_rect(
            x.saturating_add(offset),
            y,
            selected_width,
            metrics.height.min(rect.height),
            theme.surface.to_xrgb(),
        );
    }
    let caret = editor.caret.clamp(view.text, editor.text.len());
    paint_host_ui_text_parts_clipped(
        surface,
        x,
        y,
        &[
            &editor.text[view.text..caret],
            &editor.preedit[view.preedit..],
            &editor.text[caret..],
        ],
        theme.text,
        font_size,
        width,
    );
    let cells = composer::cells(&editor.text[view.text..caret])
        + composer::cells(&editor.preedit[view.preedit..]);
    surface.fill_rect(
        x.saturating_add((cells as u32 * cell).min(width.saturating_sub(1))),
        y,
        (scale as u32).max(1),
        metrics.height.min(rect.height),
        theme.accent.to_xrgb(),
    );
}

/// Bounded paragraph layout for host-owned status copy, including CJK without
/// spaces. Width and line height use the same fixed-grid metrics as painting.
fn walk_host_ui_paragraph(
    text: &str,
    width: u32,
    font_size: u16,
    mut emit: impl FnMut(&str) -> bool,
) {
    let cell = font::cell_metrics(font_size).width.max(1);
    let capacity = (width / cell) as usize;
    if capacity == 0 {
        return;
    }
    let mut line = String::new();
    let mut cells = 0;
    for (offset, character) in text.char_indices() {
        let width = composer::character_cells(character);
        let word_width = if character.is_ascii()
            && !character.is_whitespace()
            && line.ends_with(char::is_whitespace)
        {
            composer::cells(text[offset..].split_whitespace().next().unwrap_or_default())
        } else {
            0
        };
        if (word_width <= capacity && cells + word_width > capacity)
            || character == '\n'
            || cells + width > capacity
        {
            if !emit(line.trim_end()) {
                return;
            }
            line.clear();
            cells = 0;
            if character == '\n' || character.is_whitespace() {
                continue;
            }
        }
        line.push(character);
        cells += width;
    }
    let _ = emit(line.trim_end());
}
pub(crate) fn host_ui_paragraph_lines(text: &str, width: u32, font_size: u16) -> Vec<String> {
    let mut lines = Vec::new();
    walk_host_ui_paragraph(text, width, font_size, |line| {
        lines.push(line.into());
        true
    });
    lines
}
pub(crate) fn paint_host_ui_paragraph(
    surface: &mut Surface<'_>,
    rect: ui::Rect,
    text: &str,
    color: Rgb,
    font_size: u16,
) {
    let metrics = font::cell_metrics(font_size);
    let line_height = metrics.height.max(1).saturating_add(3);
    let mut y = rect.y;
    walk_host_ui_paragraph(text, rect.width, font_size, |line| {
        if y.saturating_add(metrics.height) > rect.y.saturating_add(rect.height) {
            return false;
        }
        paint_host_ui_text(surface, rect.x, y, line, color, font_size, rect.width);
        y = y.saturating_add(line_height);
        true
    });
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

/// Paints the bottom status bar: an optional left label (the active tab), an
/// optional clipboard type/size readout, and a
/// fixed-width `L###:C###` cursor readout pinned to the right. The readout keeps
/// a constant width (zero-padded, three digits) so the bar never reflows as the
/// cursor moves — the same no-jitter rule the crosshair will rely on. Rows and
/// columns are 1-based to match vt100 CUP coordinates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_status_bar(
    surface: &mut Surface<'_>,
    layout: ui::Layout,
    theme: theme::Theme,
    left_label: &str,
    clipboard: Option<&str>,
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
    // A narrow window must not paint two readouts over each other. Prefer
    // clipboard type if it fits; otherwise retain the fixed cursor position.
    let clipboard_width = clipboard.map(|label| {
        let cells: u32 = label
            .chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .sum();
        cell_w.saturating_mul(cells)
    });
    let usable_width = bar.width.saturating_sub(pad.saturating_mul(2));
    let both_fit = clipboard_width.is_some_and(|width| {
        width
            .saturating_add(dip(16.0))
            .saturating_add(readout_width)
            <= usable_width
    });
    let clipboard_only = !both_fit && clipboard_width.is_some_and(|width| width <= usable_width);
    let mut right_edge = readout_x;
    if both_fit || clipboard_only {
        let clipboard = clipboard.expect("width requires a clipboard label");
        let clipboard_width = clipboard_width.expect("label has a measured width");
        let clipboard_x = if both_fit {
            readout_x
                .saturating_sub(dip(16.0))
                .saturating_sub(clipboard_width)
        } else {
            bar.x
                .saturating_add(bar.width)
                .saturating_sub(pad)
                .saturating_sub(clipboard_width)
        };
        paint_host_ui_text(
            surface,
            clipboard_x,
            text_y,
            clipboard,
            theme.muted,
            font_size_px,
            clipboard_width,
        );
        right_edge = clipboard_x;
    }
    if !clipboard_only {
        paint_host_ui_text(
            surface,
            readout_x,
            text_y,
            &readout,
            theme.muted,
            font_size_px,
            readout_width.min(usable_width),
        );
    }
    // Left: the active tab label, clipped so it never runs into the readouts.
    if !left_label.is_empty() {
        let left_x = bar.x.saturating_add(pad);
        let left_max = right_edge.saturating_sub(dip(8.0)).saturating_sub(left_x);
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
    shortcuts: [&str; ui::SETTINGS_SHORTCUT_LINES as usize],
    font_size_px: u16,
    font_percent: u16,
    update: &crate::update::State,
    // What a Restart press costs, already rendered in `ui_language`, so the
    // painter states the loss without knowing the tab count itself. Empty when
    // there is nothing to state.
    restart_cost: &str,
    section: ui::SettingsSection,
    proxy_input: &composer::ComposerState,
    proxy_focused: bool,
    proxy_error: Option<&str>,
    proxy_options: &[String],
    list_start: usize,
) {
    let dip = |value: f64| minicon_core::numeric::round_f64(value * scale.max(1.0)).max(0.0) as u32;
    if section == ui::SettingsSection::Harness {
        return;
    }
    if section == ui::SettingsSection::Web {
        paint_web_settings(
            surface,
            layout.web_settings(scale, list_start),
            scale,
            theme,
            ui_language,
            font_size_px,
            proxy_input,
            proxy_focused,
            proxy_error,
            proxy_options,
        );
        return;
    }
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
    paint_settings_routes(surface, s.routes, section, theme, font_size_px, scale);
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
    for (rect, choice) in s.theme.iter().zip(theme::ThemeChoice::ALL) {
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
        paint_host_ui_text(
            surface,
            rect.x.saturating_add(dip(4.0)),
            text_dy(*rect).saturating_sub(dip(2.0)),
            choice.label(ui_language),
            pal.text,
            font_size_px,
            rect.width.saturating_sub(dip(8.0)),
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
    for (index, line) in shortcuts
        .into_iter()
        .take(s.shortcut_lines as usize)
        .enumerate()
    {
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

    // Updates. Three controls whose availability is the phase, and one line
    // the phase is written into, so the section is readable without the panel
    // knowing anything about the worker.
    section_label(
        surface,
        ui::Rect {
            x: inner_x,
            y: s.updates_row,
            width: inner_w,
            height: metrics.height,
        },
        strings.settings_updates,
    );
    let action = |surface: &mut Surface<'_>, rect: ui::Rect, label: &str, enabled: bool| {
        surface.fill_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            theme.surface.to_xrgb(),
        );
        paint_host_ui_text(
            surface,
            rect.x.saturating_add(dip(8.0)),
            text_dy(rect),
            label,
            if enabled { theme.text } else { theme.muted },
            font_size_px,
            rect.width,
        );
    };
    use crate::update::Phase;
    let can_check = matches!(
        update.phase,
        Phase::Idle
            | Phase::UpToDate
            | Phase::Available { .. }
            | Phase::Ahead { .. }
            | Phase::Failed { .. }
    );
    let can_install = matches!(update.phase, Phase::Available { .. });
    let can_restart = matches!(
        update.phase,
        Phase::Ready {
            opens_dmg: false,
            ..
        }
    );
    action(surface, s.update[0], strings.update_check, can_check);
    action(surface, s.update[1], strings.update_install, can_install);
    action(surface, s.update[2], strings.update_restart, can_restart);
    let status = ui_language.update_status(update);
    paint_host_ui_text(
        surface,
        inner_x,
        s.updates_status_y,
        &status,
        theme.muted,
        font_size_px,
        inner_w,
    );
    // The cost of the press, stated only while the press is the one that is
    // available: it belongs to the Restart control, and a window that cannot
    // restart has nothing to warn about.
    if can_restart && !restart_cost.is_empty() {
        paint_host_ui_text(
            surface,
            inner_x,
            s.updates_cost_y,
            restart_cost,
            theme.text,
            font_size_px,
            inner_w,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeaderIcon {
    NewRoot,
    NewWeb,
    Harness,
    Settings,
    /// Collapses the sidebar to a rail, or expands it again. Drawn as a panel
    /// with a chevron pointing the way the sidebar will move, so the glyph
    /// states the outcome rather than the current state — a control that shows
    /// where you are gives you nothing you cannot already see.
    SidebarToggle {
        collapsed: bool,
    },
    /// Collapses the right-side history/interaction panel, or expands it. The
    /// mirror of [`Self::SidebarToggle`]: the divider is the panel's inner edge
    /// and the chevron points the way that edge will travel, so the glyph states
    /// the outcome rather than the current state.
    PanelToggle {
        open: bool,
    },
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

/// Browser navigation marks use geometry so native font coverage cannot hide
/// an otherwise clickable control. The order matches the toolbar hit targets.
pub(crate) fn paint_browser_navigation_icon(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    index: usize,
    color: Rgb,
    scale: f64,
) {
    let segments = match index {
        0 => &[((10, 3), (5, 8)), ((5, 8), (10, 13)), ((5, 8), (14, 8))][..],
        1 => &[((6, 3), (11, 8)), ((11, 8), (6, 13)), ((2, 8), (11, 8))][..],
        2 => &[
            ((13, 5), (10, 2)),
            ((10, 2), (6, 2)),
            ((6, 2), (3, 5)),
            ((3, 5), (3, 10)),
            ((3, 10), (6, 13)),
            ((6, 13), (10, 13)),
            ((10, 13), (13, 10)),
            ((13, 0), (13, 5)),
            ((8, 5), (13, 5)),
        ][..],
        _ => return,
    };
    let scale = scale.clamp(1.0, 4.0);
    let unit = minicon_core::numeric::round_f64(scale).max(1.0) as u32;
    let extent = (16.0 * scale) as u32;
    let left = button.x + button.width.saturating_sub(extent) / 2;
    let top = button.y + button.height.saturating_sub(extent) / 2;
    for &((x0, y0), (x1, y1)) in segments {
        let mut x = (f64::from(x0) * scale) as i32;
        let mut y = (f64::from(y0) * scale) as i32;
        let end_x = (f64::from(x1) * scale) as i32;
        let end_y = (f64::from(y1) * scale) as i32;
        let dx = (end_x - x).abs();
        let dy = -(end_y - y).abs();
        let step_x = if x < end_x { 1 } else { -1 };
        let step_y = if y < end_y { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            surface.fill_rect(left + x as u32, top + y as u32, unit, unit, color.to_xrgb());
            if x == end_x && y == end_y {
                break;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += step_x;
            }
            if doubled <= dx {
                error += dx;
                y += step_y;
            }
        }
    }
}

/// Paints the header as one aligned, borderless icon family.
///
/// The layout rectangles remain the hit targets, but no permanent box is
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
    if button.width == 0 || button.height == 0 {
        return;
    }
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
        HeaderIcon::NewRoot | HeaderIcon::NewWeb => {
            let window = ui::Rect {
                x: button.x.saturating_add(inset),
                y: button.y.saturating_add(inset),
                width: button.width.saturating_sub(
                    inset
                        .saturating_mul(3)
                        .saturating_add(stroke.saturating_mul(4)),
                ),
                height: button.height.saturating_sub(
                    inset
                        .saturating_mul(3)
                        .saturating_add(stroke.saturating_mul(4)),
                ),
            };
            if icon == HeaderIcon::NewWeb {
                // A framed page with a toolbar reads as Web at small rail
                // sizes; it uses the same stroke and hover treatment as the
                // terminal tray and the other header icons.
                stroke_rect(surface, window, stroke, color);
                surface.fill_rect(
                    window.x,
                    window.y.saturating_add(stroke.saturating_mul(4)),
                    window.width,
                    stroke,
                    color.to_xrgb(),
                );
                surface.fill_rect(
                    window.x.saturating_add(stroke.saturating_mul(2)),
                    window.y.saturating_add(stroke.saturating_mul(2)),
                    stroke,
                    stroke,
                    color.to_xrgb(),
                );
            } else {
                // The prompt distinguishes a terminal from the browser's
                // toolbar even when both buttons shrink to rail size.
                stroke_rect(surface, window, stroke, color);
                let prompt_x = window.x.saturating_add(stroke.saturating_mul(3));
                let prompt_y = window.y.saturating_add(stroke.saturating_mul(4));
                surface.fill_rect(prompt_x, prompt_y, stroke, stroke, color.to_xrgb());
                surface.fill_rect(
                    prompt_x.saturating_add(stroke),
                    prompt_y.saturating_add(stroke),
                    stroke,
                    stroke,
                    color.to_xrgb(),
                );
                surface.fill_rect(
                    prompt_x,
                    prompt_y.saturating_add(stroke.saturating_mul(2)),
                    stroke,
                    stroke,
                    color.to_xrgb(),
                );
                surface.fill_rect(
                    prompt_x.saturating_add(stroke.saturating_mul(3)),
                    prompt_y.saturating_add(stroke.saturating_mul(2)),
                    stroke.saturating_mul(3),
                    stroke,
                    color.to_xrgb(),
                );
            }
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
        HeaderIcon::Harness => {
            let head = ui::Rect {
                x: button.x + inset,
                y: button.y + inset + stroke * 2,
                width: button.width.saturating_sub(inset * 2),
                height: button.height.saturating_sub(inset * 2 + stroke * 2),
            };
            stroke_rect(surface, head, stroke, color);
            let eyes_y = head.y + head.height / 3;
            for x in [head.x + head.width / 4, head.x + head.width * 3 / 4] {
                surface.fill_rect(x, eyes_y, stroke * 2, stroke * 2, color.to_xrgb());
            }
            surface.fill_rect(
                head.x + head.width / 3,
                head.y + head.height * 3 / 4,
                head.width / 3,
                stroke,
                color.to_xrgb(),
            );
            surface.fill_rect(
                button.x + button.width / 2,
                button.y + inset,
                stroke,
                stroke * 3,
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
        HeaderIcon::SidebarToggle { collapsed } => {
            paint_column_toggle(surface, button, stroke, color, true, collapsed);
        }
        HeaderIcon::PanelToggle { open } => {
            paint_column_toggle(surface, button, stroke, color, false, !open);
        }
    }
}

/// Draws a "column" toggle: a window frame with a solid bar marking which
/// column the control governs — left for the sidebar, right for the panel —
/// and a chevron stating the outcome, the way that column's edge will travel.
///
/// The bar is the pair's identity and never moves with state, so the two
/// toggles never read as the same control; the chevron alone carries the state,
/// keeping the family rule that a glyph shows what a click will do rather than
/// where things already are. `expand` is true when a click widens the column.
fn paint_column_toggle(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    stroke: u32,
    color: Rgb,
    bar_on_left: bool,
    expand: bool,
) {
    let inset = stroke.saturating_mul(3);
    let panel = ui::Rect {
        x: button.x.saturating_add(inset),
        y: button.y.saturating_add(inset),
        width: button.width.saturating_sub(inset.saturating_mul(2)),
        height: button.height.saturating_sub(inset.saturating_mul(2)),
    };
    stroke_rect(surface, panel, stroke, color);
    let bar_w = (panel.width / 3).max(stroke);
    let bar_x = if bar_on_left {
        panel.x
    } else {
        panel.x.saturating_add(panel.width.saturating_sub(bar_w))
    };
    surface.fill_rect(bar_x, panel.y, bar_w, panel.height, color.to_xrgb());
    // The chevron lives in the open area beside the bar, so the bar and the
    // arrow never overlap.
    let (region_left, region_right) = if bar_on_left {
        (
            bar_x
                .saturating_add(bar_w)
                .saturating_add(stroke.saturating_mul(2)),
            panel.x.saturating_add(panel.width).saturating_sub(stroke),
        )
    } else {
        (
            panel.x.saturating_add(stroke),
            bar_x.saturating_sub(stroke.saturating_mul(2)),
        )
    };
    if region_right <= region_left {
        return;
    }
    let cx = region_left.saturating_add(region_right.saturating_sub(region_left) / 2);
    let mid_y = panel.y.saturating_add(panel.height / 2);
    let arm_len = (panel.height / 4)
        .max(stroke)
        .min(region_right.saturating_sub(region_left) / 2)
        .max(stroke);
    for step in 0..arm_len {
        let reach = arm_len.saturating_sub(step);
        let dx = if expand {
            cx.saturating_add(reach)
        } else {
            cx.saturating_sub(reach)
        };
        surface.fill_rect(
            dx,
            mid_y.saturating_sub(step),
            stroke,
            stroke,
            color.to_xrgb(),
        );
        surface.fill_rect(
            dx,
            mid_y.saturating_add(step),
            stroke,
            stroke,
            color.to_xrgb(),
        );
    }
}

/// A pencil glyph for a tab row's edit button.
///
/// Drawn from axis-aligned `fill_rect` squares rather than a glyph, so it never
/// depends on a symbol font (the same rule the header icons follow). The body
/// is a diagonal run from the lower-left to the upper-right; a flat cap closes
/// the top end and a thin base marks the writing end, so it reads as a pencil
/// rather than a bare slash.
pub(crate) fn paint_edit_glyph(
    surface: &mut Surface<'_>,
    button: ui::Rect,
    color: Rgb,
    scale: f64,
) {
    let stroke = minicon_core::numeric::round_f64(scale.clamp(1.0, 4.0)).clamp(1.0, 4.0) as u32;
    let inset = stroke.saturating_mul(3);
    let inner = ui::Rect {
        x: button.x.saturating_add(inset),
        y: button.y.saturating_add(inset),
        width: button.width.saturating_sub(inset.saturating_mul(2)),
        height: button.height.saturating_sub(inset.saturating_mul(2)),
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let body = stroke.saturating_mul(2);
    let steps = inner.width.min(inner.height);
    // The pencil body: a thick diagonal from bottom-left to top-right.
    for step in 0..steps {
        let x = inner.x.saturating_add(step);
        let y = inner
            .y
            .saturating_add(inner.height)
            .saturating_sub(stroke)
            .saturating_sub(step);
        surface.fill_rect(x, y.saturating_sub(body), stroke, body, color.to_xrgb());
    }
    // A flat cap at the top-right end.
    surface.fill_rect(
        inner.x.saturating_add(steps.saturating_sub(body)),
        inner.y,
        body,
        stroke,
        color.to_xrgb(),
    );
    // A thin base at the bottom-left end, reading as the writing tip.
    surface.fill_rect(
        inner.x,
        inner.y.saturating_add(inner.height).saturating_sub(stroke),
        body,
        stroke,
        color.to_xrgb(),
    );
}

/// Width in pixels the host UI font needs for `text`.
///
/// The host UI paints on the same monospace cell grid the terminal uses, so a
/// string's width is its display columns times the cell width — wide CJK
/// characters included. Needed wherever something must be centred or sized to
/// fit its own text rather than to a guessed constant.
pub(crate) fn host_ui_text_width(text: &str, font_size_px: u16) -> u32 {
    let cell_w = font::cell_metrics(font_size_px).width.max(1);
    let columns: u32 = text
        .chars()
        .map(|character| {
            u32::try_from(unicode_width::UnicodeWidthChar::width(character).unwrap_or(0))
                .unwrap_or(0)
        })
        .sum();
    cell_w.saturating_mul(columns)
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

pub(crate) fn paint_settings_routes(
    surface: &mut Surface<'_>,
    routes: [ui::Rect; 3],
    section: ui::SettingsSection,
    theme: theme::Theme,
    size: u16,
    scale: f64,
) {
    for (index, rect) in routes.into_iter().enumerate() {
        let active = index == section.index();
        surface.fill_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            if active { theme.accent } else { theme.surface }.to_xrgb(),
        );
        paint_host_ui_text(
            surface,
            rect.x + (8.0 * scale) as u32,
            rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
            ["Console", "Web", "Harness"][index],
            if active { theme.panel_bg } else { theme.text },
            size,
            rect.width.saturating_sub((16.0 * scale) as u32),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_web_settings(
    surface: &mut Surface<'_>,
    s: ui::WebSettingsLayout,
    scale: f64,
    theme: theme::Theme,
    lang: ui::UiLanguage,
    size: u16,
    editor: &composer::ComposerState,
    focused: bool,
    error: Option<&str>,
    options: &[String],
) {
    surface.fill_rect(
        s.panel.x,
        s.panel.y,
        s.panel.width,
        s.panel.height,
        theme.panel_bg.to_xrgb(),
    );
    stroke_rect(surface, s.panel, scale.max(1.0) as u32, theme.border);
    paint_settings_routes(
        surface,
        s.routes,
        ui::SettingsSection::Web,
        theme,
        size,
        scale,
    );
    let (notice, placeholder, add, empty, previous, next) = match lang {
        ui::UiLanguage::English => (
            "Save choices here; apply beside the Browser address.",
            "http://127.0.0.1:8890",
            "Add",
            "No saved proxies. Add one above.",
            "Previous",
            "Next",
        ),
        ui::UiLanguage::ChineseSimplified => (
            "此处仅保存候选；在网页地址旁选择后生效。",
            "http://127.0.0.1:8890",
            "添加",
            "暂无候选代理，请在上方添加。",
            "上一页",
            "下一页",
        ),
        ui::UiLanguage::ChineseTraditional => (
            "此處僅儲存候選；在網頁地址旁選擇後生效。",
            "http://127.0.0.1:8890",
            "新增",
            "尚無候選代理，請在上方新增。",
            "上一頁",
            "下一頁",
        ),
    };
    paint_host_ui_paragraph(surface, s.notice, notice, theme.muted, size);
    paint_single_line_host_editor(
        surface,
        s.proxy_input,
        editor,
        focused,
        placeholder,
        theme,
        size,
        scale,
    );
    let button = |surface: &mut Surface<'_>, rect: ui::Rect, label: &str, enabled: bool| {
        surface.fill_rect(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            theme.surface.to_xrgb(),
        );
        paint_host_ui_text(
            surface,
            rect.x + (6.0 * scale) as u32,
            rect.y + rect.height.saturating_sub(font::cell_metrics(size).height) / 2,
            label,
            if enabled { theme.text } else { theme.muted },
            size,
            rect.width.saturating_sub((12.0 * scale) as u32),
        );
    };
    button(surface, s.proxy_add, add, options.len() < 8);
    if let Some(error) = error {
        paint_host_ui_text(
            surface,
            s.proxy_input.x,
            s.proxy_input.y + s.proxy_input.height + (6.0 * scale) as u32,
            error,
            theme.error,
            size,
            s.notice.width,
        );
    }
    if options.is_empty() && s.visible_rows > 0 {
        paint_host_ui_text(
            surface,
            s.proxy_rows[0].x,
            s.proxy_rows[0].y,
            empty,
            theme.muted,
            size,
            s.proxy_rows[0].width,
        );
    }
    for index in 0..s.visible_rows {
        let Some(value) = options.get(s.list_start + index) else {
            break;
        };
        let row = s.proxy_rows[index];
        let remove = s.proxy_remove[index];
        surface.fill_rect(row.x, row.y, row.width, row.height, theme.surface.to_xrgb());
        paint_host_ui_text(
            surface,
            row.x + (8.0 * scale) as u32,
            row.y + row.height.saturating_sub(font::cell_metrics(size).height) / 2,
            value,
            theme.text,
            size,
            remove.x.saturating_sub(row.x + (12.0 * scale) as u32),
        );
        button(surface, remove, "×", true);
    }
    if options.len() > s.visible_rows {
        button(surface, s.previous, previous, s.list_start > 0);
        button(
            surface,
            s.next,
            next,
            s.list_start + s.visible_rows < options.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_icons_distinguish_terminal_from_web_in_every_theme() {
        for choice in theme::ThemeChoice::ALL {
            let theme = theme::Theme::for_choice(choice);
            let paint = |icon| {
                let mut pixels = vec![theme.sidebar_bg.to_xrgb(); 40 * 40];
                paint_header_icon_button(
                    &mut Surface::new(&mut pixels, 40, 40),
                    ui::Rect {
                        x: 4,
                        y: 4,
                        width: 32,
                        height: 32,
                    },
                    icon,
                    theme.accent,
                    false,
                    theme.surface,
                    15,
                    1.0,
                );
                pixels
            };
            let terminal = paint(HeaderIcon::NewRoot);
            let web = paint(HeaderIcon::NewWeb);
            assert_ne!(terminal, web, "terminal and Web must have distinct icons");
            assert!(
                terminal
                    .iter()
                    .any(|pixel| *pixel == theme.accent.to_xrgb())
            );
            assert!(web.iter().any(|pixel| *pixel == theme.accent.to_xrgb()));
        }
    }

    #[test]
    fn clipboard_status_pixels_keep_type_readable_without_cursor_overlap() {
        let theme = theme::Theme::for_choice(theme::ThemeChoice::default());
        let compact = crate::clipboard_status::readout(
            crate::clipboard_status::ClipboardSummary {
                kind: crate::clipboard_status::ClipboardKind::TextAndImage,
                length: crate::clipboard_status::ClipboardLength::Chars(12),
            },
            false,
            true,
        )
        .expect("mixed clipboard status");
        let paint = |width: u32, cursor, label: Option<&str>| {
            let height = 320;
            let mut pixels = vec![0; (width * height) as usize];
            paint_status_bar(
                &mut Surface::new(&mut pixels, width, height),
                ui::Layout::new(width, height, 1.0),
                theme,
                "",
                label,
                cursor,
                13,
                1.0,
            );
            pixels
        };
        let narrow_with_cursor = paint(430, Some((1, 2)), Some(&compact));
        let narrow_without_cursor = paint(430, None, Some(&compact));
        assert!(
            narrow_with_cursor == narrow_without_cursor,
            "narrow bar painted cursor over clipboard type/size"
        );
        assert!(
            narrow_with_cursor != paint(430, Some((1, 2)), None),
            "narrow bar omitted the clipboard type/size"
        );
        assert!(
            paint(800, Some((1, 2)), Some(&compact)) != paint(800, None, Some(&compact)),
            "normal bar should retain the cursor readout beside clipboard status"
        );
    }

    #[test]
    fn disabled_browser_navigation_is_visibly_distinct_in_every_theme() {
        for choice in theme::ThemeChoice::ALL {
            let theme = theme::Theme::for_choice(choice);
            let active = browser_navigation_color(theme, true);
            let disabled = browser_navigation_color(theme, false);
            assert_eq!(active, theme.text);
            assert_ne!(disabled, active);
            assert_ne!(disabled, theme.sidebar_bg);
            let mut enabled_pixels = vec![theme.sidebar_bg.to_xrgb(); 34 * 44];
            let mut disabled_pixels = enabled_pixels.clone();
            paint_browser_navigation_icon(
                &mut Surface::new(&mut enabled_pixels, 34, 44),
                ui::Rect {
                    x: 0,
                    y: 0,
                    width: 34,
                    height: 44,
                },
                0,
                active,
                1.0,
            );
            paint_browser_navigation_icon(
                &mut Surface::new(&mut disabled_pixels, 34, 44),
                ui::Rect {
                    x: 0,
                    y: 0,
                    width: 34,
                    height: 44,
                },
                0,
                disabled,
                1.0,
            );
            assert_ne!(enabled_pixels, disabled_pixels);
        }
    }

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
