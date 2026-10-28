//! Host-chrome painting: the window frame MiniCon draws around the hosted
//! terminals — header, tab sidebar, composer, status strip, settings panel —
//! and the greeting surface shown when no tab exists.
//!
//! Distinct from [`crate::host_ui`], which holds the reusable drawing
//! primitives (text runs, buttons, panels) this module composes; this module
//! owns the `ConApp`-aware layout that decides what goes where.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use super::*;

fn paint_menu_scrollbar(
    surface: &mut Surface<'_>,
    choices: ui::MenuLayout,
    scale: f64,
    color: palette::Rgb,
) {
    if choices.visible_rows == 0 || choices.visible_rows >= choices.count {
        return;
    }
    let width = (3.0 * scale).max(1.0) as u32;
    let track = choices.rect.height.saturating_sub((4.0 * scale) as u32);
    let height = ((u64::from(track) * choices.visible_rows as u64) / choices.count as u64) as u32;
    let y = choices
        .rect
        .y
        .saturating_add((2.0 * scale) as u32)
        .saturating_add(
            ((u64::from(track.saturating_sub(height)) * choices.start as u64)
                / (choices.count - choices.visible_rows) as u64) as u32,
        );
    surface.fill_rect(
        choices
            .rect
            .x
            .saturating_add(choices.rect.width)
            .saturating_sub(width + 1),
        y,
        width,
        height.max(1),
        color.to_xrgb(),
    );
}

impl ConApp {
    /// The zoom level the status bar reports, as a percentage of the session's
    /// starting font size.
    ///
    /// Both status bars — the one under a terminal and the one under the
    /// greeting page — showed this, each with its own copy of the same
    /// expression and the same `100` fallback. Two copies of a number that must
    /// never disagree is one copy too many.
    pub(super) fn font_zoom_percent(&self) -> u16 {
        self.active_session_opt().map_or(100, |session| {
            ((session.font_size_logical / session.font_size_baseline.max(1.0)) * 100.0).round()
                as u16
        })
    }

    fn paint_new_tab_menu(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        scale: f64,
        font_logical: f64,
    ) {
        if !self.new_tab_menu_open {
            return;
        }
        let theme = theme::Theme::for_choice(self.ui_theme);
        let choices = layout.new_tab_choices(scale, self.new_menu_scroll);
        let menu = choices.rect;
        surface.fill_rect(
            menu.x,
            menu.y,
            menu.width,
            menu.height,
            theme.panel_bg.to_xrgb(),
        );
        stroke_rect(surface, menu, scale.max(1.0) as u32, theme.border);
        let (terminal, browser) = match self.ui_language {
            ui::UiLanguage::English => ("Terminal", "Browser"),
            ui::UiLanguage::ChineseSimplified => ("终端", "网页"),
            ui::UiLanguage::ChineseTraditional => ("終端", "網頁"),
        };
        let label_size = scaled_host_ui_font(16, font_logical, scale);
        let hint_size = scaled_host_ui_font(11, font_logical, scale);
        for index in choices.start..choices.start + choices.visible_rows {
            let rect = choices.row(index);
            let (label, hint) = if index == 0 {
                (terminal, "Ctrl+Shift+T")
            } else {
                (browser, "Ctrl+Shift+O")
            };
            if index == self.new_menu_highlight {
                surface.fill_rect(
                    rect.x,
                    rect.y,
                    rect.width,
                    rect.height,
                    theme.surface.to_xrgb(),
                );
            }
            let pad = (8.0 * scale) as u32;
            let x = rect.x.saturating_add(pad);
            let width = rect.width.saturating_sub(pad.saturating_mul(2));
            let label_h = font::cell_metrics(label_size).height;
            let hint_h = font::cell_metrics(hint_size).height;
            let show_hint = rect.height >= label_h.saturating_add(hint_h).saturating_add(4);
            let block_h = label_h.saturating_add(if show_hint {
                hint_h.saturating_add(2)
            } else {
                0
            });
            let top = rect
                .y
                .saturating_add(rect.height.saturating_sub(block_h) / 2);
            paint_host_ui_text(surface, x, top, label, theme.text, label_size, width);
            if show_hint {
                paint_host_ui_text(
                    surface,
                    x,
                    top.saturating_add(label_h).saturating_add(2),
                    hint,
                    theme.muted,
                    hint_size,
                    width,
                );
            }
        }
        paint_menu_scrollbar(surface, choices, scale, theme.accent);
    }

    fn paint_browser_recovery(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        scale: f64,
        title: &str,
        detail: &str,
        proxy_failure: bool,
    ) {
        let theme = theme::Theme::for_choice(self.ui_theme);
        let card = layout.browser_recovery_card(scale);
        surface.fill_rect(
            card.x,
            card.y,
            card.width,
            card.height,
            theme.panel_bg.to_xrgb(),
        );
        stroke_rect(surface, card, scale.max(1.0) as u32, theme.border);
        let pad = (16.0 * scale) as u32;
        let retry = layout.browser_retry(scale);
        let edit = layout.browser_edit_address(scale);
        let size =
            |nominal| scaled_host_ui_font(nominal, self.session_seed.font_size_logical(), scale);
        let title_height = (48.0 * scale) as u32;
        let text_x = card.x.saturating_add(pad.min(card.width / 4));
        let text_y = card.y.saturating_add(pad.min(card.height / 8));
        let text_width = card
            .width
            .saturating_sub(pad.min(card.width / 4).saturating_mul(2));
        let text_bottom = retry.y.saturating_sub((10.0 * scale) as u32);
        let title_height = title_height.min(text_bottom.saturating_sub(text_y));
        host_ui::paint_host_ui_paragraph(
            surface,
            ui::Rect {
                x: text_x,
                y: text_y,
                width: text_width,
                height: title_height,
            },
            title,
            theme.text,
            size(18),
        );
        let detail_y = text_y
            .saturating_add(title_height)
            .saturating_add((4.0 * scale) as u32);
        host_ui::paint_host_ui_paragraph(
            surface,
            ui::Rect {
                x: text_x,
                y: detail_y,
                width: text_width,
                height: text_bottom.saturating_sub(detail_y),
            },
            detail,
            theme.muted,
            size(14),
        );
        for (rect, label, border) in [
            (
                retry,
                browser_feedback::retry_label(self.ui_language),
                theme.accent,
            ),
            (
                edit,
                if proxy_failure {
                    browser_feedback::edit_proxy_label(self.ui_language)
                } else {
                    browser_feedback::edit_label(self.ui_language)
                },
                theme.border,
            ),
        ] {
            surface.fill_rect(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                theme.surface.to_xrgb(),
            );
            stroke_rect(surface, rect, scale.max(1.0) as u32, border);
            paint_button_label(surface, rect, label, theme.text, size(15));
        }
    }

    pub(crate) fn paint_host_ui(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        candidate: DirtyRegion,
    ) -> Result<(), PixelWindowError> {
        let session = self.active_session()?;
        let clip = candidate_bounds(candidate, width, height);
        let mut surface = Surface::with_clip(pixels, width, height, clip);
        let scale = session.scale.max(1.0);
        let layout = self.layout(width, height, scale);
        let tree_width = layout.sidebar.width;
        let host_ui_size = |nominal| scaled_host_ui_font(nominal, session.font_size_logical, scale);
        // The host UI (sidebar, tabs, composer, borders) is themed; the
        // terminal body keeps its ANSI/xterm colors regardless. Only these
        // outer surfaces change when the user switches themes.
        let t = theme::Theme::for_choice(self.ui_theme);
        let tree_bg = t.sidebar_bg;
        let tree_rule = t.border;
        let active_bg = t.surface;
        let accent = t.accent;
        let error_accent = t.error;
        let composer_bg = t.composer_bg;
        let text = t.text;
        let muted = t.muted;
        // The terminal column's right edge: the window's, less the history
        // panel. The composer and status bands stop here so neither slides under
        // the panel.
        let column_right = if layout.panel_open {
            layout.panel.x
        } else {
            width
        };
        surface.fill_rect(0, 0, tree_width, height, tree_bg.to_xrgb());
        surface.fill_rect(
            tree_width.saturating_sub(1),
            0,
            1,
            height,
            tree_rule.to_xrgb(),
        );
        surface.fill_rect(
            tree_width,
            height.saturating_sub(session.content_bottom_px),
            column_right.saturating_sub(tree_width),
            session.content_bottom_px,
            composer_bg.to_xrgb(),
        );
        surface.fill_rect(
            tree_width,
            height.saturating_sub(session.content_bottom_px),
            column_right.saturating_sub(tree_width),
            1,
            tree_rule.to_xrgb(),
        );

        self.paint_workspace_chrome(
            &mut surface,
            layout,
            width,
            height,
            scale,
            session.font_size_logical,
        );

        let active_id = self.workspace.active().map(|id| id.get()).unwrap_or(0);
        let input_y = layout.composer.y;
        let header_x = tree_width.saturating_add(12);
        let ime_width = (layout.composer.width / 2).min(260);
        let ime_x = layout
            .composer_send
            .x
            .saturating_sub(ime_width.saturating_add(8));
        let mut active_id_text = itoa::Buffer::new();
        // One status line carries the most important recoverable refusal. A
        // host notice (a tab that could not open) outranks a clipboard refusal;
        // both are temporary and actionable, and neither should stay invisible
        // just because it only reached `ui-snapshot`.
        let strip_notice = status_strip_notice(
            self.host_notice.as_deref(),
            self.terminal_clipboard_error.as_deref(),
        );
        if let Some(notice) = strip_notice {
            paint_host_ui_text(
                &mut surface,
                header_x,
                input_y + 7,
                notice,
                error_accent,
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                ime_x.saturating_sub(header_x.saturating_add(8)),
            );
        } else {
            // Names the tab, not just its number — and by the same label the
            // history rows record (`active_destination_label`): a custom
            // rename or the program stem, not the shell's changing OSC title.
            // The band and the history saying different names for one send is
            // the drift this shared owner closes (2026-10-21 inventory).
            let destination = self.active_destination_label();
            paint_host_ui_text_parts(
                &mut surface,
                header_x,
                input_y + 7,
                &[
                    self.ui_language.strings().send_to,
                    active_id_text.format(active_id),
                    " ",
                    destination.as_str(),
                ],
                if self.composer.submit_error.is_some() {
                    error_accent
                } else if self.composer.focused {
                    accent
                } else {
                    muted
                },
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                ime_x.saturating_sub(header_x.saturating_add(8)),
            );
        }
        paint_host_ui_text(
            &mut surface,
            ime_x,
            input_y + 7,
            &self.ime_status_label,
            if self.ime_status.as_ref().is_some_and(|status| status.open) {
                accent
            } else {
                muted
            },
            host_ui_size(HOST_UI_STATUS_SIZE_PX),
            ime_width,
        );
        surface.fill_rect(
            layout.composer_input.x,
            layout.composer_input.y,
            layout.composer_input.width,
            layout.composer_input.height,
            if self.composer.submit_error.is_some() {
                error_accent
            } else if self.composer.focused {
                accent
            } else {
                tree_rule
            }
            .to_xrgb(),
        );
        surface.fill_rect(
            layout.composer_input.x + 1,
            layout.composer_input.y + 1,
            layout.composer_input.width.saturating_sub(2),
            layout.composer_input.height.saturating_sub(2),
            // The selection is drawn per row over the text below, so the box
            // itself keeps the plain composer background.
            composer_bg.to_xrgb(),
        );
        // Both buttons get the same plate. Filling only one left the other as
        // floating text with no edge -- it read as a label rather than
        // something to press, which is the whole difference a button makes.
        for button in [
            layout.composer_send,
            layout.composer_newline,
            layout.composer_copy,
            layout.composer_paste,
            layout.composer_cut,
        ] {
            surface.fill_rect(
                button.x,
                button.y,
                button.width,
                button.height,
                active_bg.to_xrgb(),
            );
        }
        let composer_selection = composer::selection_bounds(&self.composer);
        let show_caret = self.composer.focused && composer_selection.is_none();
        // Each stored newline owns a real painted row. The fixed-height input
        // follows the caret's row, while each row retains the existing
        // horizontal sliding window for commands wider than the box.
        let composer_font_size = host_ui_size(COMPOSER_TEXT_SIZE_PX);
        let composer_metrics = font::cell_metrics(composer_font_size);
        let composer_cell_width = composer_metrics.width.max(1);
        let composer_line_height = composer_metrics.height.max(1);
        let composer_text_width = layout
            .composer_scrollbar
            .x
            .saturating_sub(layout.composer_input.x)
            .saturating_sub(COMPOSER_TEXT_INSET.saturating_mul(2));
        let composer_cells = (composer_text_width / composer_cell_width) as usize;
        let rows =
            (layout.composer_input.height.saturating_sub(8) / composer_line_height).max(1) as usize;
        let lines = self.composer_visible_lines(rows);
        let scrollbar = ui::composer_scrollbar_geometry(
            layout,
            rows,
            lines.first_line,
            composer::line_count(&self.composer.text),
        );
        let track = scrollbar.track;
        let thumb = scrollbar.thumb;
        surface.fill_rect(
            track.left.max(0) as u32,
            track.top.max(0) as u32,
            track.width().max(0) as u32,
            track.height().max(0) as u32,
            muted.to_xrgb(),
        );
        surface.fill_rect(
            thumb.left.max(0) as u32,
            thumb.top.max(0) as u32,
            thumb.width().max(0) as u32,
            thumb.height().max(0) as u32,
            accent.to_xrgb(),
        );
        let caret_line = composer::line_index_at(&self.composer.text, self.composer.caret);
        for row in 0..lines.line_count {
            let line_index = lines.first_line + row;
            let range = composer::line_range(&self.composer.text, line_index);
            let line = &self.composer.text[range.clone()];
            let is_caret_line = line_index == caret_line;
            let local_caret = if is_caret_line {
                self.composer
                    .caret
                    .saturating_sub(range.start)
                    .min(line.len())
            } else {
                line.len()
            };
            let preedit = if is_caret_line {
                self.composer.preedit.as_str()
            } else {
                ""
            };
            let window = composer::visible_window(
                line,
                preedit,
                local_caret,
                usize::from(show_caret && is_caret_line),
                composer_cells,
            );
            let caret = local_caret.clamp(window.text, line.len());
            let caret_cells = usize::from(window.truncated)
                + composer::cells(&line[window.text..caret])
                + composer::cells(&preedit[window.preedit..]);
            let y = layout
                .composer_input
                .y
                .saturating_add(4)
                .saturating_add(composer_line_height.saturating_mul(row as u32));
            // Selection highlight for this row, drawn under the text: intersect
            // the global selection with the line, clamp to the visible window,
            // and fill the selected cells (a truncation "…" shifts cells by one).
            if let Some((sel_start, sel_end)) = composer_selection {
                let row_start = sel_start.max(range.start).min(range.end);
                let row_end = sel_end.max(range.start).min(range.end);
                let vis_start = (row_start - range.start).max(window.text);
                let vis_end = (row_end - range.start).max(window.text);
                if vis_end > vis_start {
                    let lead = usize::from(window.truncated);
                    let start_cell = lead + composer::cells(&line[window.text..vis_start]);
                    let end_cell = lead + composer::cells(&line[window.text..vis_end]);
                    let x = layout.composer_input.x
                        + COMPOSER_TEXT_INSET
                        + composer_cell_width.saturating_mul(start_cell as u32);
                    let w = composer_cell_width
                        .saturating_mul((end_cell - start_cell) as u32)
                        .min(composer_text_width.saturating_sub(
                            x.saturating_sub(layout.composer_input.x + COMPOSER_TEXT_INSET),
                        ));
                    surface.fill_rect(x, y, w, composer_line_height, active_bg.to_xrgb());
                }
            }
            paint_host_ui_text_parts(
                &mut surface,
                layout.composer_input.x + COMPOSER_TEXT_INSET,
                y,
                &[
                    if window.truncated { "…" } else { "" },
                    &line[window.text..caret],
                    &preedit[window.preedit..],
                    &line[caret..],
                ],
                text,
                composer_font_size,
                composer_text_width,
            );
            // Drawn as a rule rather than a character so hit-testing and text
            // columns remain identical.
            if show_caret && is_caret_line {
                let offset = composer_cell_width
                    .saturating_mul(caret_cells as u32)
                    .min(composer_text_width.saturating_sub(1));
                surface.fill_rect(
                    layout.composer_input.x + COMPOSER_TEXT_INSET + offset,
                    y,
                    2,
                    composer_line_height,
                    accent.to_xrgb(),
                );
            }
        }
        let strings = self.ui_language.strings();
        // Centre each two-line block as a unit, then centre each line on its
        // own. The smaller shortcut line fits the stacked controls without
        // growing the composer band or taking width from the draft.
        paint_two_line_button_label(
            &mut surface,
            layout.composer_send,
            strings.send,
            strings.send_hint,
            if self.composer.submit_error.is_some() {
                error_accent
            } else {
                accent
            },
            host_ui_size(BUTTON_LABEL_SIZE_PX),
            host_ui_size(BUTTON_HINT_SIZE_PX),
        );
        paint_two_line_button_label(
            &mut surface,
            layout.composer_newline,
            strings.newline,
            strings.newline_hint,
            accent,
            host_ui_size(BUTTON_LABEL_SIZE_PX),
            host_ui_size(BUTTON_HINT_SIZE_PX),
        );
        // Copy / Paste / Cut buttons. Copy and Cut need a selection to do
        // anything, so they dim to the muted tone when nothing is selected;
        // Paste is always live. Zero-width (hidden on a narrow composer)
        // buttons paint nothing, so no guard is needed here.
        let has_selection = composer::selection_bounds(&self.composer).is_some();
        let clip_label_size = host_ui_size(BUTTON_HINT_SIZE_PX);
        let selection_tone = if has_selection { text } else { muted };
        paint_button_label(
            &mut surface,
            layout.composer_copy,
            strings.copy,
            selection_tone,
            clip_label_size,
        );
        paint_button_label(
            &mut surface,
            layout.composer_paste,
            strings.paste,
            text,
            clip_label_size,
        );
        paint_button_label(
            &mut surface,
            layout.composer_cut,
            strings.cut,
            selection_tone,
            clip_label_size,
        );
        // The status readout follows the grid crosshair (the hovered cell) when
        // the pointer has been over the terminal, and falls back to the text
        // cursor otherwise.
        let status_cursor = self.active_session_opt().map(|session| {
            if session.crosshair_active
                && let Some(cell) = session.crosshair_cell
            {
                (cell.row, cell.col)
            } else {
                session.parser.screen().cursor_position()
            }
        });
        let status_label = self
            .active_session_opt()
            .map_or("", |session| session.current_title.as_str());
        let clipboard = self.clipboard_readout(layout.status.width);
        paint_status_bar(
            &mut surface,
            layout,
            t,
            status_label,
            clipboard.as_deref(),
            status_cursor,
            host_ui_size(HOST_UI_STATUS_SIZE_PX),
            scale,
        );
        if self.settings_open {
            let restart_cost = self.ui_language.restart_cost(self.workspace.nodes().len());
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                self.font_zoom_percent(),
                &self.update,
                &restart_cost,
                self.settings_section,
                &self.settings_proxy_input,
                self.settings_proxy_focused,
                self.settings_proxy_error.as_deref(),
                &self.config.browser_proxy_options,
                self.settings_proxy_list_start,
            );
            self.paint_harness_settings(
                &mut surface,
                layout,
                scale,
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
            );
        }
        self.paint_new_tab_menu(&mut surface, layout, scale, session.font_size_logical);
        Ok(())
    }

    /// Paints the right-side history panel: a title, the newest sent lines as
    /// clickable rows, and a blank area reserved for future loop/goal controls.
    ///
    /// The list is a VIEW over the composer's own sent-history store — the same
    /// entries Up/Down recall — so a row here and a recall at the same offset
    /// are the same line.
    fn paint_history_panel(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        scale: f64,
        t: theme::Theme,
        font_size_px: u16,
    ) {
        let p = layout.history_panel(scale);
        let strings = self.ui_language.strings();
        let pad = minicon_core::numeric::round_f64(12.0 * scale.max(1.0)).max(0.0) as u32;
        let inner_w = p.panel.width.saturating_sub(pad.saturating_mul(2));
        surface.fill_rect(
            p.panel.x,
            0,
            p.panel.width,
            p.panel.height,
            t.sidebar_bg.to_xrgb(),
        );
        surface.fill_rect(p.panel.x, 0, 1, p.panel.height, t.border.to_xrgb());
        paint_host_ui_text(
            surface,
            p.panel.x.saturating_add(pad),
            p.title_y,
            strings.history_title,
            t.muted,
            font_size_px,
            inner_w,
        );
        // A quiet close mark in the panel's top-right corner.
        paint_host_ui_text(
            surface,
            p.close.x,
            p.title_y,
            "x",
            t.muted,
            font_size_px,
            p.close.width,
        );
        let history = self.composer.history();
        // One clamped offset for paint, hit test and snapshot — see
        // `history_visible_offset`. Read here rather than trusting the stored
        // one: a resize can shrink the capacity under it, and a paint whose row
        // indexing rested on a stale offset would read past the list.
        let offset = self.history_visible_offset(p.row_capacity);
        let visible = history.len().saturating_sub(offset).min(p.row_capacity);
        if visible == 0 {
            paint_host_ui_text(
                surface,
                p.panel.x.saturating_add(pad),
                p.list_top,
                strings.history_empty,
                t.muted,
                font_size_px,
                inner_w,
            );
        } else {
            let cell_w = font::cell_metrics(font_size_px).width.max(1);
            let gap = cell_w.saturating_mul(2);
            for index in 0..visible {
                let entry = &history[history.len() - 1 - offset - index];
                let first_line = entry.text.lines().next().unwrap_or("");
                let y = p
                    .list_top
                    .saturating_add((index as u32).saturating_mul(p.row_height));
                // The destination is right-aligned in muted ink and the line
                // keeps the rest, so a row says which tab it came from without
                // spending its leading cells on it. A label wide enough to
                // crowd the line yields the whole row instead of eating it.
                let label_width = u32::try_from(entry.destination.chars().count())
                    .unwrap_or(u32::MAX)
                    .saturating_mul(cell_w);
                let show_label = !entry.destination.is_empty() && label_width < inner_w;
                let text_width = if show_label {
                    inner_w.saturating_sub(label_width).saturating_sub(gap)
                } else {
                    inner_w
                };
                paint_host_ui_text(
                    surface,
                    p.panel.x.saturating_add(pad),
                    y,
                    first_line,
                    t.text,
                    font_size_px,
                    text_width,
                );
                if show_label {
                    paint_host_ui_text(
                        surface,
                        p.panel
                            .x
                            .saturating_add(pad)
                            .saturating_add(text_width)
                            .saturating_add(gap),
                        y,
                        &entry.destination,
                        t.muted,
                        font_size_px,
                        inner_w.saturating_sub(text_width).saturating_sub(gap),
                    );
                }
            }
        }
        // The notes section is marked off by the same rule that used to bound
        // the blank reservation, so the panel still reads as deliberate
        // sections rather than content that ran out.
        surface.fill_rect(
            p.panel.x.saturating_add(pad),
            p.reserved_top,
            inner_w,
            1,
            t.border.to_xrgb(),
        );
        // The notes scratchpad: a title, then the multiline text with a caret
        // while focused. Overflow is clipped (no own scroll — the panel's
        // wheel belongs to the history list above), and the empty state is a
        // quiet hint rather than blank space.
        paint_host_ui_text(
            surface,
            p.panel.x.saturating_add(pad),
            p.notes.title_y,
            strings.notes_title,
            t.muted,
            font_size_px,
            inner_w,
        );
        let cell = font::cell_metrics(font_size_px);
        let cell_w = cell.width.max(1);
        let cell_h = cell.height.max(1);
        let line_gap = (cell_h / 4).max(1);
        let line_stride = cell_h.saturating_add(line_gap);
        let note_cols = (p.notes.text_w / cell_w).max(1) as usize;
        let note_lines = p.notes_visible_rows;
        if self.notes.text.is_empty() && self.notes.preedit.is_empty() {
            if self.notes_focused || !strings.notes_empty.is_empty() {
                paint_host_ui_text(
                    surface,
                    p.notes.text_x,
                    p.notes.text_y,
                    strings.notes_empty,
                    t.muted,
                    font_size_px,
                    p.notes.text_w,
                );
            }
        } else {
            // The composer's own line painter wraps the same way the composer
            // panel does; rows beyond the visible window are clipped, showing
            // the tail of the note (where writing happens).
            let lines: Vec<&str> = self.notes.text.lines().collect();
            let preedit_first = self.notes.preedit.lines().next().unwrap_or("");
            // The view offset is clamped at every read (the history offset's
            // rule): a resize that shrank the window cannot leave the view
            // pointing past the note's own head.
            let max_back = lines.len().saturating_sub(note_lines);
            let skip = self.notes_scroll.min(max_back);
            for (index, line) in lines.iter().enumerate().skip(skip) {
                let visible_index = index - skip;
                let y = p
                    .notes
                    .text_y
                    .saturating_add((visible_index as u32).saturating_mul(line_stride));
                // The preedit renders on the caret's line; a simpler contract
                // than the composer's (which windows long lines) because a
                // note line wraps at the panel width instead of scrolling.
                let mut parts: [&str; 2] = [line, ""];
                let mut tail = "";
                if self.notes_focused && !preedit_first.is_empty() {
                    let caret_line = composer::line_index_at(
                        &self.notes.text,
                        self.notes.caret.min(self.notes.text.len()),
                    );
                    if caret_line == index {
                        let caret = self.notes.caret.min(self.notes.text.len());
                        let line_start = self
                            .notes
                            .text
                            .lines()
                            .take(index)
                            .map(|l| l.len() + 1)
                            .sum::<usize>();
                        let offset = caret.saturating_sub(line_start);
                        let split = offset.min(line.len());
                        parts = [&line[..split], &line[split..]];
                        tail = preedit_first;
                        let _ = note_cols; // width bound is the panel's
                    }
                }
                let mut segments: [&str; 3] = [parts[0], parts[1], tail];
                if tail.is_empty() {
                    segments = [parts[0], parts[1], ""];
                }
                paint_host_ui_text_parts(
                    surface,
                    p.notes.text_x,
                    y,
                    &segments,
                    t.text,
                    font_size_px,
                    p.notes.text_w,
                );
            }
        }
        // The caret is a rule, not a character (the composer's own rule),
        // drawn only while the notes own the keyboard.
        if self.notes_focused {
            let caret = self.notes.caret.min(self.notes.text.len());
            let line = composer::line_index_at(&self.notes.text, caret);
            let lines: Vec<&str> = self.notes.text.lines().collect();
            let max_back = lines.len().saturating_sub(note_lines);
            let skip = self.notes_scroll.min(max_back);
            if line >= skip {
                let line_start = self
                    .notes
                    .text
                    .lines()
                    .take(line)
                    .map(|l| l.len() + 1)
                    .sum::<usize>();
                let offset = caret.saturating_sub(line_start);
                let head = lines.get(line).map_or("", |l| l.get(..offset).unwrap_or(l));
                let caret_cells = composer::cells(head);
                let x = p
                    .notes
                    .text_x
                    .saturating_add(cell_w.saturating_mul(caret_cells as u32));
                let y = p.notes.text_y.saturating_add(
                    (u32::try_from(line - skip).unwrap_or(u32::MAX)).saturating_mul(line_stride),
                );
                surface.fill_rect(x, y, 1.max(cell_w / 8), cell_h, t.text.to_xrgb());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_workspace_chrome(
        &self,
        surface: &mut Surface<'_>,
        layout: ui::Layout,
        width: u32,
        height: u32,
        scale: f64,
        font_logical: f64,
    ) {
        let t = theme::Theme::for_choice(self.ui_theme);
        let tree_width = layout.sidebar.width;
        let header_height = layout.tree_header_height;
        let row_height = layout.tree_row_height;
        let host_ui_size = |nominal| scaled_host_ui_font(nominal, font_logical, scale);
        let tree_rule = t.border;
        let branch = t.branch;
        let composer_bg = t.composer_bg;
        let active_bg = t.surface;
        let accent = t.accent;
        let error_accent = t.error;
        let text = t.text;
        let muted = t.muted;
        let header_icon_size = host_ui_size(HOST_UI_HEADER_SIZE_PX);
        self.paint_harness_entry(surface, layout, scale, header_icon_size);
        paint_header_icon_button(
            surface,
            layout.new_root,
            HeaderIcon::NewRoot,
            accent,
            self.hovered_header == Some(HeaderIcon::NewRoot),
            t.surface,
            header_icon_size,
            scale,
        );
        paint_header_icon_button(
            surface,
            layout.new_web,
            HeaderIcon::NewWeb,
            accent,
            self.hovered_header == Some(HeaderIcon::NewWeb),
            t.surface,
            header_icon_size,
            scale,
        );
        paint_header_icon_button(
            surface,
            layout.settings,
            HeaderIcon::Settings,
            accent,
            self.settings_open || self.hovered_header == Some(HeaderIcon::Settings),
            t.surface,
            header_icon_size,
            scale,
        );
        let toggle_icon = HeaderIcon::SidebarToggle {
            collapsed: layout.sidebar_collapsed,
        };
        paint_header_icon_button(
            surface,
            layout.sidebar_toggle,
            toggle_icon,
            accent,
            self.hovered_header == Some(toggle_icon),
            t.surface,
            header_icon_size,
            scale,
        );
        let panel_icon = HeaderIcon::PanelToggle {
            open: layout.panel_open,
        };
        paint_header_icon_button(
            surface,
            layout.panel_toggle,
            panel_icon,
            accent,
            self.hovered_header == Some(panel_icon),
            t.surface,
            header_icon_size,
            scale,
        );
        let home = layout.home;
        if home.height > 0 {
            surface.fill_rect(
                home.x,
                home.y,
                home.width,
                home.height,
                if self.home_active {
                    t.surface
                } else {
                    t.sidebar_bg
                }
                .to_xrgb(),
            );
            if self.home_active {
                surface.fill_rect(home.x, home.y, 3, home.height, t.accent.to_xrgb());
            }
            let label = if layout.sidebar_collapsed {
                "@0"
            } else {
                match self.ui_language {
                    ui::UiLanguage::English => "@0  Home",
                    ui::UiLanguage::ChineseSimplified => "@0  首页",
                    ui::UiLanguage::ChineseTraditional => "@0  首頁",
                }
            };
            paint_host_ui_text(
                surface,
                home.x + (12.0 * scale) as u32,
                home.y
                    + (home.height.saturating_sub(
                        font::cell_metrics(host_ui_size(HOST_UI_TAB_SIZE_PX)).height,
                    )) / 2,
                label,
                t.text,
                host_ui_size(HOST_UI_TAB_SIZE_PX),
                home.width.saturating_sub((18.0 * scale) as u32),
            );
        }
        if layout.panel_open {
            self.paint_history_panel(
                surface,
                layout,
                scale,
                t,
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
            );
        }
        // A rail row shows `@N` and nothing else, so the hovered row's full
        // label has to be drawn somewhere; collected here and painted after the
        // rows so it floats above them.
        let mut rail_tooltip: Option<(u32, String)> = None;

        let nodes = self.workspace.nodes();
        let depths = self.workspace.depths();
        for (visible_index, (node_index, node)) in nodes
            .iter()
            .enumerate()
            .skip(self.tree_scroll_offset)
            .enumerate()
        {
            let y = header_height + visible_index as u32 * row_height;
            if y >= height {
                break;
            }
            let depth = depths.get(node_index).copied().unwrap_or(0).min(8);
            let indent = 14 + depth * 18;
            if self.workspace.active() == Some(node.id) {
                surface.fill_rect(0, y, tree_width, row_height, active_bg.to_xrgb());
                surface.fill_rect(0, y, 3, row_height, accent.to_xrgb());
            }
            if depth > 0 {
                let branch_x = indent.saturating_sub(10);
                surface.fill_rect(branch_x, y, 1, row_height / 2 + 1, branch.to_xrgb());
                surface.fill_rect(branch_x, y + row_height / 2, 8, 1, branch.to_xrgb());
            }
            let session = self.session_for(node.id);
            // Two lines per row. The name line is the tab's own label: the
            // user's name when set, otherwise the program it runs (`cmd`, ...),
            // and finally the node's creation-time label. The title line below
            // is the terminal's OSC title — what `title` (Windows) or
            // `printf '\e]0;…'` sets — which is a separate thing and tracks the
            // shell live.
            let browser_title = self.browsers.get(&node.id).map(|tab| {
                let title = if tab.panel().title.is_empty() {
                    "Browser"
                } else {
                    &tab.panel().title
                };
                title.to_owned()
            });
            let name = tree_row_name(
                node.custom_title.as_deref(),
                session.map(|terminal| terminal.program_label()),
                browser_title.as_deref().unwrap_or(node.title.as_str()),
            );
            let browser_description = self.browsers.get(&node.id).map(|tab| {
                match (&tab.panel().load, self.ui_language) {
                    (browser::PageLoad::Loading { .. }, ui::UiLanguage::English) => {
                        "Web · Loading…"
                    }
                    (browser::PageLoad::Unavailable { .. }, ui::UiLanguage::English) => {
                        "Web · Unable to load"
                    }
                    (_, ui::UiLanguage::English) => "Web page",
                    (browser::PageLoad::Loading { .. }, ui::UiLanguage::ChineseSimplified) => {
                        "网页 · 正在加载"
                    }
                    (browser::PageLoad::Unavailable { .. }, ui::UiLanguage::ChineseSimplified) => {
                        "网页 · 加载失败"
                    }
                    (_, ui::UiLanguage::ChineseSimplified) => "网页",
                    (browser::PageLoad::Loading { .. }, ui::UiLanguage::ChineseTraditional) => {
                        "網頁 · 載入中"
                    }
                    (browser::PageLoad::Unavailable { .. }, ui::UiLanguage::ChineseTraditional) => {
                        "網頁 · 載入失敗"
                    }
                    (_, ui::UiLanguage::ChineseTraditional) => "網頁",
                }
            });
            let title = browser_description.or_else(|| {
                tree_row_title(session.map(|terminal| terminal.current_title.as_str()))
            });
            // A child that exited leaves its tab in place (remain-on-exit), so
            // the row is the only thing that can say the shell is gone. Dim the
            // label: it reads as inert next to a live tab without moving or
            // recolouring the row a user is aiming at.
            // A tab whose shell exited non-zero reads in the error color so a
            // crash is visible at a glance; a clean exit dims to muted; a live
            // shell uses the normal text color.
            let label_color = match session {
                Some(terminal) if terminal.child_gone => {
                    if terminal.child_exit_code.unwrap_or(0) != 0 {
                        error_accent
                    } else {
                        muted
                    }
                }
                _ => text,
            };
            let mut id = itoa::Buffer::new();
            let row_active = self.workspace.active() == Some(node.id);
            let row_hovered = self.hovered_tree_row == Some(node_index);
            if layout.sidebar_collapsed {
                // The rail identifies a tab by its stable `@ID` — the same
                // handle the control CLI uses — so what a user reads here is
                // what they would type. Depth, branch lines and the close
                // button are dropped: none of them survive a 44dip column
                // legibly, and a half-drawn close button is worse than none.
                let label = ["@", id.format(node.id.get())].concat();
                let font_size = host_ui_size(HOST_UI_TAB_SIZE_PX);
                let text_width = host_ui_text_width(&label, font_size);
                let x = tree_width.saturating_sub(text_width) / 2;
                let text_h = font::cell_metrics(font_size).height.max(1);
                paint_host_ui_text(
                    surface,
                    x,
                    y.saturating_add(row_height.saturating_sub(text_h) / 2),
                    &label,
                    label_color,
                    font_size,
                    tree_width,
                );
                if row_hovered {
                    rail_tooltip = Some((visible_index as u32, name.to_owned()));
                }
                continue;
            }
            // While this row is being renamed, the label gives way to an inline
            // editor and the row buttons are hidden so the text gets the width.
            if let Some(rename) = self
                .rename
                .as_ref()
                .filter(|rename| rename.target == node.id)
            {
                let font_size = host_ui_size(HOST_UI_TAB_SIZE_PX);
                let metrics = font::cell_metrics(font_size);
                let cell_w = metrics.width.max(1);
                let cell_h = metrics.height.max(1);
                let geometry = layout.tree_rename_geometry(visible_index, depth, cell_h);
                let avail = geometry.text_width;
                if avail > 0 {
                    let editor = &rename.editor;
                    let cells = (avail / cell_w).max(1) as usize;
                    let window = composer::visible_window(
                        &editor.text,
                        &editor.preedit,
                        editor.caret,
                        1,
                        cells,
                    );
                    let caret = editor.caret.clamp(window.text, editor.text.len());
                    // Centre the single-line editor in the taller row; while the
                    // rename is live there is no title line to sit under.
                    let plate_y = geometry.plate.y;
                    let plate_h = geometry.plate.height;
                    let text_y = geometry.text_y;
                    surface.fill_rect(
                        geometry.plate.x,
                        plate_y,
                        geometry.plate.width,
                        plate_h,
                        composer_bg.to_xrgb(),
                    );
                    let lead = usize::from(window.truncated);
                    let cell_x = |cells_used: usize| {
                        geometry
                            .text_x
                            .saturating_add(cell_w.saturating_mul((lead + cells_used) as u32))
                    };
                    if window.truncated {
                        paint_host_ui_text(
                            surface,
                            geometry.text_x,
                            text_y,
                            "…",
                            text,
                            font_size,
                            cell_w,
                        );
                    }
                    // A selection is drawn as an inverted span — accent plate
                    // with the text in the editor background color — rather than
                    // the composer's subtle bg fill, because on entry the whole
                    // name is selected and a low-contrast tint read as "nothing
                    // happened". The inversion is theme-independent (accent vs
                    // bg always contrast). The IME preedit suppresses it, since
                    // the span offsets would no longer line up.
                    let selection = editor
                        .preedit
                        .is_empty()
                        .then(|| composer::selection_bounds(editor))
                        .flatten();
                    match selection {
                        Some((sel_start, sel_end)) => {
                            let vs = sel_start.max(window.text).min(editor.text.len());
                            let ve = sel_end.max(window.text).min(editor.text.len());
                            let head = &editor.text[window.text..vs];
                            let mid = &editor.text[vs..ve];
                            let tail = &editor.text[ve..];
                            let head_cells = composer::cells(head);
                            let mid_cells = composer::cells(mid);
                            if !head.is_empty() {
                                paint_host_ui_text(
                                    surface,
                                    cell_x(0),
                                    text_y,
                                    head,
                                    text,
                                    font_size,
                                    avail,
                                );
                            }
                            if mid_cells > 0 {
                                let sx = cell_x(head_cells);
                                let width = cell_w.saturating_mul(mid_cells as u32);
                                surface.fill_rect(sx, plate_y, width, plate_h, accent.to_xrgb());
                                paint_host_ui_text(
                                    surface,
                                    sx,
                                    text_y,
                                    mid,
                                    composer_bg,
                                    font_size,
                                    width,
                                );
                            }
                            if !tail.is_empty() {
                                paint_host_ui_text(
                                    surface,
                                    cell_x(head_cells.saturating_add(mid_cells)),
                                    text_y,
                                    tail,
                                    text,
                                    font_size,
                                    avail,
                                );
                            }
                        }
                        None => {
                            paint_host_ui_text_parts(
                                surface,
                                indent,
                                text_y,
                                &[
                                    &editor.text[window.text..caret],
                                    &editor.preedit[window.preedit..],
                                    &editor.text[caret..],
                                ],
                                text,
                                font_size,
                                avail,
                            );
                        }
                    }
                    // The caret is a rule, not a character, so text columns stay
                    // exact — the same choice the composer makes.
                    let caret_cells = lead
                        + composer::cells(&editor.text[window.text..caret])
                        + composer::cells(&editor.preedit[window.preedit..]);
                    let offset = cell_w
                        .saturating_mul(caret_cells as u32)
                        .min(avail.saturating_sub(1));
                    surface.fill_rect(
                        indent.saturating_add(offset),
                        plate_y,
                        2,
                        plate_h,
                        accent.to_xrgb(),
                    );
                }
            } else {
                // Two stacked lines, centred as a block in the row: the name on
                // top, the terminal's title under it in the muted color.
                let name_font = host_ui_size(HOST_UI_TAB_SIZE_PX);
                let title_font = host_ui_size(if browser_description.is_some() {
                    12
                } else {
                    HOST_UI_SUBTITLE_SIZE_PX
                });
                let name_h = font::cell_metrics(name_font).height.max(1);
                let title_h = font::cell_metrics(title_font).height.max(1);
                let block_h = name_h.saturating_add(if title.is_some() { title_h } else { 0 });
                let top = y.saturating_add(row_height.saturating_sub(block_h) / 2);
                // Three row buttons (edit, new-child, close) now sit at the
                // right edge; keep the label clipped clear of all of them.
                let text_width = tree_width.saturating_sub(indent).saturating_sub(82);
                paint_host_ui_text_parts_clipped(
                    surface,
                    indent,
                    top,
                    &["@", id.format(node.id.get()), "  ", name],
                    label_color,
                    name_font,
                    text_width,
                );
                if let Some(title) = title {
                    paint_host_ui_text_parts_clipped(
                        surface,
                        indent,
                        top.saturating_add(name_h),
                        &[title],
                        muted,
                        title_font,
                        text_width,
                    );
                }
                if row_active || row_hovered {
                    let edit = layout.tree_edit_rect(visible_index, scale);
                    let child = layout.tree_child_rect(visible_index, scale);
                    let close = layout.tree_close_rect(visible_index, scale);
                    // A hover plate behind each glyph so it reads as a button,
                    // not floating text.
                    if row_hovered {
                        for plate in [edit, child, close] {
                            surface.fill_rect(
                                plate.x,
                                plate.y,
                                plate.width,
                                plate.height,
                                active_bg.to_xrgb(),
                            );
                        }
                    }
                    paint_edit_glyph(surface, edit, text, scale);
                    // The new-child button between edit and close: the same
                    // "+" the header's New uses, scoped to this row
                    // (Ctrl+Shift+N's pointer twin).
                    paint_host_ui_text(
                        surface,
                        child.x + 6,
                        child.y + 3,
                        "+",
                        text,
                        host_ui_size(HOST_UI_CLOSE_SIZE_PX),
                        child.width.saturating_sub(6),
                    );
                    paint_host_ui_text(
                        surface,
                        close.x + 6,
                        close.y + 3,
                        "x",
                        text,
                        host_ui_size(HOST_UI_CLOSE_SIZE_PX),
                        close.width.saturating_sub(6),
                    );
                }
            }
        }

        if let Some(drag) = self.tree_drag.filter(|drag| drag.dragging) {
            let capacity = layout.tree_capacity();
            let row_y = |index: usize| {
                index
                    .checked_sub(self.tree_scroll_offset)
                    .filter(|&visible| visible <= capacity)
                    .map(|visible| {
                        header_height
                            .saturating_add(row_height.saturating_mul(visible as u32))
                            .min(layout.sidebar.height.saturating_sub(3))
                    })
            };
            if let Some(source_index) = nodes.iter().position(|node| node.id == drag.source) {
                let source_depth = depths[source_index];
                let branch_len = 1 + depths[source_index + 1..]
                    .iter()
                    .take_while(|&&depth| depth > source_depth)
                    .count();
                if source_index
                    .checked_sub(self.tree_scroll_offset)
                    .is_some_and(|visible| visible < capacity)
                    && let Some(y) = row_y(source_index)
                {
                    surface.fill_rect(0, y, 4, row_height, muted.to_xrgb());
                    let ghost = format!("{branch_len} tabs");
                    paint_host_ui_text(
                        surface,
                        tree_width.saturating_sub(84),
                        y.saturating_add(2),
                        &ghost,
                        muted,
                        host_ui_size(HOST_UI_STATUS_SIZE_PX),
                        80,
                    );
                }
            }
            if let Some(proposal) = drag.proposal {
                let color = if drag.refusal.is_some() {
                    error_accent
                } else {
                    accent
                };
                match proposal {
                    workspace::MovePlacement::RootTail => {
                        if let Some(y) = row_y(nodes.len()) {
                            surface.fill_rect(
                                4,
                                y,
                                tree_width.saturating_sub(8),
                                3,
                                color.to_xrgb(),
                            );
                        }
                    }
                    workspace::MovePlacement::Before(target)
                    | workspace::MovePlacement::After(target)
                    | workspace::MovePlacement::Into(target) => {
                        if let Some(index) = nodes.iter().position(|node| node.id == target) {
                            match proposal {
                                workspace::MovePlacement::Before(_) => {
                                    if let Some(y) = row_y(index) {
                                        surface.fill_rect(
                                            4,
                                            y,
                                            tree_width.saturating_sub(8),
                                            3,
                                            color.to_xrgb(),
                                        );
                                    }
                                }
                                workspace::MovePlacement::Into(_) => {
                                    if let Some(y) = row_y(index) {
                                        stroke_rect(
                                            surface,
                                            ui::Rect {
                                                x: 4,
                                                y,
                                                width: tree_width.saturating_sub(8),
                                                height: row_height,
                                            },
                                            2,
                                            color,
                                        );
                                    }
                                }
                                workspace::MovePlacement::After(_) => {
                                    let depth = depths[index];
                                    let end = index
                                        + 1
                                        + depths[index + 1..]
                                            .iter()
                                            .take_while(|&&candidate| candidate > depth)
                                            .count();
                                    if let Some(y) = row_y(end) {
                                        surface.fill_rect(
                                            4,
                                            y,
                                            tree_width.saturating_sub(8),
                                            3,
                                            color.to_xrgb(),
                                        );
                                    }
                                }
                                workspace::MovePlacement::RootTail => unreachable!(),
                            }
                        }
                    }
                }
            }
        }

        if let Some((visible_row, title)) = rail_tooltip {
            let tip_size = host_ui_size(HOST_UI_TAB_SIZE_PX);
            let text_width = host_ui_text_width(&title, tip_size);
            if let Some(tip) = ui::rail_tooltip_rect(layout, visible_row, text_width, width, scale)
            {
                surface.fill_rect(tip.x, tip.y, tip.width, tip.height, active_bg.to_xrgb());
                stroke_rect(surface, tip, 1, tree_rule);
                let padding = tip.width.saturating_sub(text_width) / 2;
                paint_host_ui_text(
                    surface,
                    tip.x.saturating_add(padding),
                    tip.y.saturating_add(tip.height / 2).saturating_sub(7),
                    &title,
                    text,
                    tip_size,
                    tip.width.saturating_sub(padding),
                );
            }
        }
    }

    /// Browser tab shell: same sidebar as the terminal host, one content panel.
    /// A native child WebView covers the content rectangle when available; this
    /// paint remains the typed Unavailable surface. The proxy line is the
    /// reserved hook, not a setting.
    pub(crate) fn paint_browser_shell(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        scale: f64,
    ) {
        let mut surface = Surface::with_clip(
            pixels,
            width,
            height,
            PixelRect::from_xywh(0, 0, width, height),
        );
        let layout = self.layout(width, height, scale);
        let t = theme::Theme::for_choice(self.ui_theme);
        let scale = scale.max(1.0);
        let host_ui_size =
            |nominal| scaled_host_ui_font(nominal, self.session_seed.font_size_logical(), scale);
        surface.fill_rect(0, 0, width, height, t.canvas_bg.to_xrgb());
        surface.fill_rect(0, 0, layout.sidebar.width, height, t.sidebar_bg.to_xrgb());
        surface.fill_rect(
            layout.sidebar.width.saturating_sub(1),
            0,
            1,
            height,
            t.border.to_xrgb(),
        );
        self.paint_workspace_chrome(
            &mut surface,
            layout,
            width,
            height,
            scale,
            self.session_seed.font_size_logical(),
        );
        let toolbar = layout.browser_toolbar(scale);
        let address = layout.browser_address(scale);
        surface.fill_rect(
            toolbar.x,
            toolbar.y,
            toolbar.width,
            toolbar.height,
            t.sidebar_bg.to_xrgb(),
        );
        let size = host_ui_size(16);
        for (index, enabled) in [self.browser_can_back, self.browser_can_forward, true]
            .into_iter()
            .enumerate()
        {
            host_ui::paint_browser_navigation_icon(
                &mut surface,
                ui::Rect {
                    x: toolbar.x + (index as f64 * 34.0 * scale) as u32,
                    y: toolbar.y,
                    width: (34.0 * scale) as u32,
                    height: ((44.0 * scale) as u32).min(toolbar.height),
                },
                index,
                host_ui::browser_navigation_color(t, enabled),
                scale,
            );
        }
        surface.fill_rect(
            address.x,
            address.y,
            address.width,
            address.height,
            t.canvas_bg.to_xrgb(),
        );
        stroke_rect(
            &mut surface,
            address,
            (scale as u32).max(1),
            if self.browser_address_focused
                && !self.new_tab_menu_open
                && !self.browser_proxy_menu_open
            {
                t.accent
            } else {
                t.border
            },
        );
        let tab = self
            .workspace
            .active()
            .and_then(|id| self.browsers.get(&id));
        let url = tab.and_then(|tab| tab.panel().url.as_deref()).unwrap_or("");
        let text_x = address.x + (8.0 * scale) as u32;
        let text_y = address.y
            + address
                .height
                .saturating_sub(font::cell_metrics(size).height)
                / 2;
        let avail = address.width.saturating_sub((16.0 * scale) as u32);
        if let Some(editor) = &self.browser_address
            && self.browser_address_focused
            && !self.new_tab_menu_open
            && !self.browser_proxy_menu_open
        {
            let cell_w = font::cell_metrics(size).width.max(1);
            let view = composer::visible_window(
                &editor.text,
                &editor.preedit,
                editor.caret,
                1,
                (avail / cell_w).max(1) as usize,
            );
            if let Some((start, end)) = composer::selection_bounds(editor) {
                let start = start.max(view.text);
                let end = end.max(view.text);
                let x =
                    (composer::cells(&editor.text[view.text..start]) as u32 * cell_w).min(avail);
                let width = (composer::cells(&editor.text[start..end]) as u32 * cell_w)
                    .min(avail.saturating_sub(x));
                surface.fill_rect(
                    text_x + x,
                    text_y,
                    width,
                    (18.0 * scale) as u32,
                    t.surface.to_xrgb(),
                );
            }
            let caret = editor.caret.clamp(view.text, editor.text.len());
            let displayed = format!(
                "{}{}{}",
                &editor.text[view.text..caret],
                &editor.preedit[view.preedit..],
                &editor.text[caret..]
            );
            paint_host_ui_text(
                &mut surface,
                text_x,
                text_y,
                &displayed,
                t.text,
                size,
                avail,
            );
            let caret = editor.caret.clamp(view.text, editor.text.len());
            let cells = composer::cells(&editor.text[view.text..caret])
                + composer::cells(&editor.preedit[view.preedit..]);
            surface.fill_rect(
                text_x + (cells as u32 * cell_w).min(avail),
                text_y,
                1.max(scale as u32),
                (18.0 * scale) as u32,
                t.accent.to_xrgb(),
            );
        } else {
            paint_host_ui_text(
                &mut surface,
                text_x,
                text_y,
                if url.is_empty() { "Enter a URL" } else { url },
                if url.is_empty() { t.muted } else { t.text },
                size,
                avail,
            );
        }
        let proxy_placeholder = match self.ui_language {
            ui::UiLanguage::English => "Proxy (Enter)",
            ui::UiLanguage::ChineseSimplified => "代理（回车应用）",
            ui::UiLanguage::ChineseTraditional => "代理（Enter 套用）",
        };
        host_ui::paint_single_line_host_editor(
            &mut surface,
            layout.browser_proxy_text(scale),
            &self.browser_proxy_input,
            self.browser_proxy_focused
                && !self.browser_proxy_menu_open
                && !self.new_tab_menu_open
                && !self.settings_open,
            proxy_placeholder,
            t,
            size,
            scale,
        );
        let dropdown = layout.browser_proxy_dropdown(scale);
        surface.fill_rect(
            dropdown.x,
            dropdown.y,
            dropdown.width,
            dropdown.height,
            t.surface.to_xrgb(),
        );
        paint_host_ui_text(
            &mut surface,
            dropdown.x + (8.0 * scale) as u32,
            dropdown.y + (6.0 * scale) as u32,
            "▾",
            t.text,
            size,
            dropdown.width.saturating_sub((8.0 * scale) as u32),
        );
        if self.browser_chrome_trigger == Some(super::browser_menu::ChromeTrigger::Proxy)
            && !self.browser_proxy_menu_open
            && !self.new_tab_menu_open
        {
            stroke_rect(&mut surface, dropdown, scale.max(1.0) as u32, t.accent);
        }
        if let Some(tab) = tab {
            let feedback = browser_feedback::for_load(&tab.panel().load, self.ui_language);
            if feedback.failed {
                self.paint_browser_recovery(
                    &mut surface,
                    layout,
                    scale,
                    feedback.title,
                    feedback.detail,
                    browser_feedback::is_proxy_failure(&tab.panel().load),
                );
            } else if matches!(tab.panel().load, browser::PageLoad::Loading { .. }) {
                surface.fill_rect(
                    toolbar.x,
                    toolbar.height.saturating_sub((2.0 * scale) as u32),
                    toolbar.width,
                    (2.0 * scale) as u32,
                    t.accent.to_xrgb(),
                );
            }
        }
        if self.browser_proxy_menu_open {
            let choices = layout.browser_proxy_menu(
                scale,
                self.config.browser_proxy_options.len() + 1,
                self.proxy_menu_scroll,
            );
            let menu = choices.rect;
            surface.fill_rect(
                menu.x,
                menu.y,
                menu.width,
                menu.height,
                t.panel_bg.to_xrgb(),
            );
            stroke_rect(&mut surface, menu, scale.max(1.0) as u32, t.border);
            let configured = match self.ui_language {
                ui::UiLanguage::English => "Use configured proxy",
                ui::UiLanguage::ChineseSimplified => "使用配置的代理",
                ui::UiLanguage::ChineseTraditional => "使用設定的代理",
            };
            for index in choices.start..choices.start + choices.visible_rows {
                let row = choices.row(index);
                if index == self.proxy_menu_highlight {
                    surface.fill_rect(row.x, row.y, row.width, row.height, t.surface.to_xrgb());
                }
                let text = if index == 0 {
                    configured
                } else {
                    &self.config.browser_proxy_options[index - 1]
                };
                let applied = if index == 0 {
                    self.browser_proxy_applied.is_empty()
                } else {
                    self.config.browser_proxy_options[index - 1] == self.browser_proxy_applied
                };
                let label = format!("{}{}", if applied { "✓ " } else { "  " }, text);
                paint_host_ui_text(
                    &mut surface,
                    row.x + (8.0 * scale) as u32,
                    row.y + row.height.saturating_sub(font::cell_metrics(size).height) / 2,
                    &label,
                    t.text,
                    size,
                    row.width.saturating_sub((16.0 * scale) as u32),
                );
            }
            paint_menu_scrollbar(&mut surface, choices, scale, t.accent);
        }
        if self.settings_open {
            let restart_cost = self.ui_language.restart_cost(self.workspace.nodes().len());
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                self.font_zoom_percent(),
                &self.update,
                &restart_cost,
                self.settings_section,
                &self.settings_proxy_input,
                self.settings_proxy_focused,
                self.settings_proxy_error.as_deref(),
                &self.config.browser_proxy_options,
                self.settings_proxy_list_start,
            );
            self.paint_harness_settings(
                &mut surface,
                layout,
                scale,
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
            );
        }
        self.paint_new_tab_menu(
            &mut surface,
            layout,
            scale,
            self.session_seed.font_size_logical(),
        );
    }

    pub(crate) fn paint_home_workspace(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        scale: f64,
    ) {
        let mut surface = Surface::with_clip(
            pixels,
            width,
            height,
            PixelRect::from_xywh(0, 0, width, height),
        );
        let layout = self.layout(width, height, scale);
        let t = theme::Theme::for_choice(self.ui_theme);
        surface.fill_rect(0, 0, width, height, t.canvas_bg.to_xrgb());
        surface.fill_rect(0, 0, layout.sidebar.width, height, t.sidebar_bg.to_xrgb());
        self.paint_workspace_chrome(
            &mut surface,
            layout,
            width,
            height,
            scale,
            self.session_seed.font_size_logical(),
        );
        let left = layout.sidebar.width;
        let content_width = width.saturating_sub(left);
        surface.fill_rect(left, 0, content_width, height, t.canvas_bg.to_xrgb());
        let pad = (28.0 * scale).max(12.0) as u32;
        let text_x = left.saturating_add(pad);
        let text_width = content_width.saturating_sub(pad.saturating_mul(2));
        let title_size = scaled_host_ui_font(28, self.session_seed.font_size_logical(), scale);
        let body_size = scaled_host_ui_font(15, self.session_seed.font_size_logical(), scale);
        let top = (height / 7).max((24.0 * scale) as u32);
        paint_host_ui_text(
            &mut surface,
            text_x,
            top,
            "MiniCon",
            t.accent,
            title_size,
            text_width,
        );
        let (slogan, intro, new_terminal, new_web) = match self.ui_language {
            ui::UiLanguage::English => (
                "Stable. Fast. Local.",
                "A calm place for your terminals and agents.",
                "New Terminal",
                "New Web",
            ),
            ui::UiLanguage::ChineseSimplified => (
                "稳定 · 高速 · 本地",
                "让终端与智能体在同一个可靠空间工作。",
                "新终端",
                "新网页",
            ),
            ui::UiLanguage::ChineseTraditional => (
                "穩定 · 高速 · 本地",
                "讓終端與智能體在同一個可靠空間工作。",
                "新終端",
                "新網頁",
            ),
        };
        let intro = if text_width < (300.0 * scale) as u32 {
            match self.ui_language {
                ui::UiLanguage::English => "Your tabs, in one place.",
                ui::UiLanguage::ChineseSimplified => "终端与智能体，一目了然。",
                ui::UiLanguage::ChineseTraditional => "終端與智能體，一目了然。",
            }
        } else {
            intro
        };
        let line = font::cell_metrics(body_size)
            .height
            .saturating_add((10.0 * scale) as u32);
        paint_host_ui_text(
            &mut surface,
            text_x,
            top.saturating_add(line.saturating_mul(2)),
            slogan,
            t.text,
            body_size,
            text_width,
        );
        paint_host_ui_text(
            &mut surface,
            text_x,
            top.saturating_add(line.saturating_mul(3)),
            intro,
            t.muted,
            body_size,
            text_width,
        );
        let actions = layout.home_actions(width, height, scale);
        for (rect, label) in actions.into_iter().zip([new_terminal, new_web]) {
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            surface.fill_rect(rect.x, rect.y, rect.width, rect.height, t.surface.to_xrgb());
            stroke_rect(&mut surface, rect, scale.max(1.0) as u32, t.accent);
            paint_host_ui_text(
                &mut surface,
                rect.x.saturating_add((12.0 * scale) as u32),
                rect.y.saturating_add((8.0 * scale) as u32),
                label,
                t.text,
                body_size,
                rect.width.saturating_sub((24.0 * scale) as u32),
            );
        }
        let card = layout.home_fleet(width, height, scale);
        if card.width > 0 && card.height > 0 {
            surface.fill_rect(card.x, card.y, card.width, card.height, t.surface.to_xrgb());
            stroke_rect(&mut surface, card, scale.max(1.0) as u32, t.border);
            let nodes = self.home_visible_nodes();
            let total = self.workspace.nodes().len();
            let exited = self
                .workspace
                .nodes()
                .iter()
                .filter(|node| {
                    self.session_for(node.id)
                        .is_some_and(|session| session.child_gone)
                })
                .count();
            let capacity = layout.home_fleet_capacity(width, height, scale);
            let start = ui::clamp_tree_scroll(self.home_list_start, nodes.len(), capacity);
            let end = start.saturating_add(capacity).min(nodes.len());
            let compact_header = card.width < (360.0 * scale) as u32;
            let heading = match (self.ui_language, compact_header) {
                (ui::UiLanguage::English, true) => {
                    format!("{}/{} tabs", nodes.len(), total)
                }
                (ui::UiLanguage::ChineseSimplified, true) => {
                    format!("标签 {}/{}", nodes.len(), total)
                }
                (ui::UiLanguage::ChineseTraditional, true) => {
                    format!("分頁 {}/{}", nodes.len(), total)
                }
                (ui::UiLanguage::English, false) => format!(
                    "Tabs {}/{} · Exited {} · Showing {}–{}",
                    nodes.len(),
                    total,
                    exited,
                    start.saturating_add(1).min(nodes.len()),
                    end
                ),
                (ui::UiLanguage::ChineseSimplified, false) => format!(
                    "标签页 {}/{} · 已退出 {} · 显示 {}–{}",
                    nodes.len(),
                    total,
                    exited,
                    start.saturating_add(1).min(nodes.len()),
                    end
                ),
                (ui::UiLanguage::ChineseTraditional, false) => format!(
                    "分頁 {}/{} · 已結束 {} · 顯示 {}–{}",
                    nodes.len(),
                    total,
                    exited,
                    start.saturating_add(1).min(nodes.len()),
                    end
                ),
            };
            let filter = layout.home_filter_action(width, height, scale);
            paint_host_ui_text(
                &mut surface,
                card.x.saturating_add((12.0 * scale) as u32),
                card.y.saturating_add((12.0 * scale) as u32),
                &heading,
                t.text,
                body_size,
                card.width
                    .saturating_sub((36.0 * scale) as u32)
                    .saturating_sub(filter.width),
            );
            if filter.width > 0 && filter.height > 0 {
                surface.fill_rect(
                    filter.x,
                    filter.y,
                    filter.width,
                    filter.height,
                    t.canvas_bg.to_xrgb(),
                );
                stroke_rect(&mut surface, filter, 1, t.border);
                let filter_label = match (self.ui_language, self.home_kind_filter) {
                    (ui::UiLanguage::English, HomeKindFilter::All) => "All ›",
                    (ui::UiLanguage::English, HomeKindFilter::Console) => "Console ›",
                    (ui::UiLanguage::English, HomeKindFilter::Web) => "Web ›",
                    (ui::UiLanguage::ChineseSimplified, HomeKindFilter::All) => "全部 ›",
                    (ui::UiLanguage::ChineseSimplified, HomeKindFilter::Console) => "终端 ›",
                    (ui::UiLanguage::ChineseSimplified, HomeKindFilter::Web) => "网页 ›",
                    (ui::UiLanguage::ChineseTraditional, HomeKindFilter::All) => "全部 ›",
                    (ui::UiLanguage::ChineseTraditional, HomeKindFilter::Console) => "終端 ›",
                    (ui::UiLanguage::ChineseTraditional, HomeKindFilter::Web) => "網頁 ›",
                };
                paint_host_ui_text(
                    &mut surface,
                    filter.x.saturating_add((5.0 * scale) as u32),
                    filter.y.saturating_add((4.0 * scale) as u32),
                    filter_label,
                    t.text,
                    scaled_host_ui_font(11, self.session_seed.font_size_logical(), scale),
                    filter.width.saturating_sub((10.0 * scale) as u32),
                );
            }
            if let Some(attention) = self.home_exit_attention().first().copied() {
                let age = attention.observed_at.and_then(|at| {
                    SystemTime::now()
                        .duration_since(at)
                        .ok()
                        .map(|elapsed| elapsed.as_secs())
                });
                let age = age.map_or_else(
                    || "?".to_owned(),
                    |seconds| {
                        if seconds < 60 {
                            format!("{seconds}s")
                        } else {
                            format!("{}m", seconds / 60)
                        }
                    },
                );
                let label = match self.ui_language {
                    ui::UiLanguage::English => {
                        format!("@{} exited · {age} ago · Inspect", attention.tab.get())
                    }
                    ui::UiLanguage::ChineseSimplified => {
                        format!("@{} 已退出 · {age}前 · 检查", attention.tab.get())
                    }
                    ui::UiLanguage::ChineseTraditional => {
                        format!("@{} 已結束 · {age}前 · 檢查", attention.tab.get())
                    }
                };
                let action = layout.home_attention_action(width, height, scale);
                paint_host_ui_text(
                    &mut surface,
                    action.x,
                    action.y,
                    &label,
                    t.accent,
                    scaled_host_ui_font(12, self.session_seed.font_size_logical(), scale),
                    action.width,
                );
            }
            let search_placeholder = match self.ui_language {
                ui::UiLanguage::English => "Search name or @ID",
                ui::UiLanguage::ChineseSimplified => "搜索名称或 @ID",
                ui::UiLanguage::ChineseTraditional => "搜尋名稱或 @ID",
            };
            host_ui::paint_single_line_host_editor(
                &mut surface,
                layout.home_search(width, height, scale),
                &self.home_search,
                self.home_search.focused,
                search_placeholder,
                t,
                scaled_host_ui_font(12, self.session_seed.font_size_logical(), scale),
                scale,
            );
            if nodes.is_empty() {
                let empty = if total > 0 {
                    match self.ui_language {
                        ui::UiLanguage::English => {
                            "No tabs match this filter. Select All to see every tab."
                        }
                        ui::UiLanguage::ChineseSimplified => {
                            "此筛选下没有标签页。选择“全部”查看所有标签。"
                        }
                        ui::UiLanguage::ChineseTraditional => {
                            "此篩選下沒有分頁。選擇「全部」查看所有分頁。"
                        }
                    }
                } else {
                    match self.ui_language {
                        ui::UiLanguage::English => {
                            "No tabs yet. Create a Terminal or Web tab above."
                        }
                        ui::UiLanguage::ChineseSimplified => {
                            "还没有标签页。可使用上方按钮新建终端或网页。"
                        }
                        ui::UiLanguage::ChineseTraditional => {
                            "尚無分頁。可使用上方按鈕新增終端或網頁。"
                        }
                    }
                };
                paint_host_ui_text(
                    &mut surface,
                    card.x.saturating_add((12.0 * scale) as u32),
                    card.y.saturating_add((84.0 * scale) as u32),
                    empty,
                    t.muted,
                    body_size,
                    card.width.saturating_sub((24.0 * scale) as u32),
                );
            }
            for (visible, node) in nodes.iter().skip(start).take(capacity).enumerate() {
                let row = layout.home_fleet_row(width, height, scale, visible);
                if row.height == 0 {
                    break;
                }
                if self.home_selected_id == Some(node.id) {
                    surface.fill_rect(row.x, row.y, row.width, row.height, t.canvas_bg.to_xrgb());
                }
                surface.fill_rect(row.x, row.y, row.width, 1, t.border.to_xrgb());
                let session = self.session_for(node.id);
                let name = tree_row_name(
                    node.custom_title.as_deref(),
                    session.map(|terminal| terminal.program_label()),
                    &node.title,
                );
                let label = format!("@{}  {}", node.id.get(), name);
                let exited = session.map(|terminal| terminal.child_gone);
                let status = if row.width < (260.0 * scale) as u32 {
                    match (self.ui_language, node.kind, exited) {
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(true)) => {
                            "Exited · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(false)) => {
                            "Active · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, None) => {
                            "Child ? · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Browser, _) => {
                            "Web · Agent unavailable"
                        }
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "已退出 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "活动中 · Agent 未知",
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Terminal, None) => {
                            "子进程未知 · Agent 未知"
                        }
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Browser, _) => {
                            "网页 · Agent 不可用"
                        }
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "已結束 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "活動中 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            None,
                        ) => "子程序未知 · Agent 未知",
                        (ui::UiLanguage::ChineseTraditional, workspace::TabKind::Browser, _) => {
                            "網頁 · Agent 不可用"
                        }
                    }
                } else {
                    match (self.ui_language, node.kind, exited) {
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(true)) => {
                            "Console · exited · agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(false)) => {
                            "Console · child active · agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, None) => {
                            "Console · status unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Browser, _) => {
                            "Web · agent status unavailable"
                        }
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "终端 · 已退出 · Agent 状态未知",
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "终端 · 子进程活动中 · Agent 状态未知",
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Terminal, None) => {
                            "终端 · 状态未知"
                        }
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Browser, _) => {
                            "网页 · Agent 状态不可用"
                        }
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "終端 · 已結束 · Agent 狀態未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "終端 · 子程序活動中 · Agent 狀態未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            None,
                        ) => "終端 · 狀態未知",
                        (ui::UiLanguage::ChineseTraditional, workspace::TabKind::Browser, _) => {
                            "網頁 · Agent 狀態不可用"
                        }
                    }
                };
                let inset = (12.0 * scale) as u32;
                paint_host_ui_text(
                    &mut surface,
                    row.x.saturating_add(inset),
                    row.y.saturating_add((4.0 * scale) as u32),
                    &label,
                    t.text,
                    body_size,
                    row.width.saturating_sub(inset.saturating_mul(2)),
                );
                paint_host_ui_text(
                    &mut surface,
                    row.x.saturating_add(inset),
                    row.y.saturating_add((25.0 * scale) as u32),
                    status,
                    t.muted,
                    scaled_host_ui_font(12, self.session_seed.font_size_logical(), scale),
                    row.width.saturating_sub(inset.saturating_mul(2)),
                );
            }
            if nodes.len() > capacity && capacity > 0 {
                let scrollbar = ui::home_list_scrollbar_geometry(
                    layout,
                    width,
                    height,
                    scale,
                    start,
                    nodes.len(),
                );
                surface.fill_rect(
                    scrollbar.track.left.max(0) as u32,
                    scrollbar.track.top.max(0) as u32,
                    scrollbar.track.width().max(0) as u32,
                    scrollbar.track.height().max(0) as u32,
                    t.canvas_bg.to_xrgb(),
                );
                surface.fill_rect(
                    scrollbar.thumb.left.max(0) as u32,
                    scrollbar.thumb.top.max(0) as u32,
                    scrollbar.thumb.width().max(0) as u32,
                    scrollbar.thumb.height().max(0) as u32,
                    t.accent.to_xrgb(),
                );
            }
            let inspector = layout.home_inspector(width, height, scale);
            if inspector.height > 0 {
                surface.fill_rect(
                    inspector.x,
                    inspector.y,
                    inspector.width,
                    inspector.height,
                    t.canvas_bg.to_xrgb(),
                );
                surface.fill_rect(
                    inspector.x,
                    inspector.y,
                    inspector.width,
                    1,
                    t.border.to_xrgb(),
                );
                let selected = self.home_selected_id.and_then(|id| {
                    self.workspace.node(id).map(|node| {
                        let session = self.session_for(id);
                        let name = tree_row_name(
                            node.custom_title.as_deref(),
                            session.map(|terminal| terminal.program_label()),
                            &node.title,
                        );
                        (
                            id,
                            node.kind,
                            name.to_owned(),
                            session.map(|s| s.child_gone),
                        )
                    })
                });
                let (heading, detail, live) = if let Some((id, kind, name, child_gone)) = selected {
                    let detail = match (self.ui_language, kind, child_gone) {
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(true)) => {
                            "Exited · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, Some(false)) => {
                            "Active · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Terminal, None) => {
                            "Child ? · Agent unknown"
                        }
                        (ui::UiLanguage::English, workspace::TabKind::Browser, _) => {
                            "Web · Agent unavailable"
                        }
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "已退出 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseSimplified,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "活动中 · Agent 未知",
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Terminal, None) => {
                            "子进程? · Agent 未知"
                        }
                        (ui::UiLanguage::ChineseSimplified, workspace::TabKind::Browser, _) => {
                            "网页 · Agent 不可用"
                        }
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(true),
                        ) => "已結束 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            Some(false),
                        ) => "活動中 · Agent 未知",
                        (
                            ui::UiLanguage::ChineseTraditional,
                            workspace::TabKind::Terminal,
                            None,
                        ) => "子程序? · Agent 未知",
                        (ui::UiLanguage::ChineseTraditional, workspace::TabKind::Browser, _) => {
                            "網頁 · Agent 不可用"
                        }
                    };
                    (format!("@{}  {name}", id.get()), detail, true)
                } else if let Some(id) = self.home_selected_id {
                    let detail = match self.ui_language {
                        ui::UiLanguage::English => "Select another tab to continue.",
                        ui::UiLanguage::ChineseSimplified => "请选择其他标签页继续。",
                        ui::UiLanguage::ChineseTraditional => "請選擇其他分頁繼續。",
                    };
                    let heading = match self.ui_language {
                        ui::UiLanguage::English => format!("@{}  Tab no longer exists", id.get()),
                        ui::UiLanguage::ChineseSimplified => format!("@{}  标签页已关闭", id.get()),
                        ui::UiLanguage::ChineseTraditional => format!("@{}  分頁已關閉", id.get()),
                    };
                    (heading, detail, false)
                } else {
                    let heading = match self.ui_language {
                        ui::UiLanguage::English => "Select a tab to inspect",
                        ui::UiLanguage::ChineseSimplified => "选择标签页查看详情",
                        ui::UiLanguage::ChineseTraditional => "選擇分頁查看詳情",
                    };
                    (heading.to_owned(), "", false)
                };
                let inset = (12.0 * scale) as u32;
                paint_host_ui_text(
                    &mut surface,
                    inspector.x.saturating_add(inset),
                    inspector.y.saturating_add((8.0 * scale) as u32),
                    &heading,
                    t.text,
                    body_size,
                    inspector.width.saturating_sub(inset.saturating_mul(2)),
                );
                if !detail.is_empty() {
                    paint_host_ui_text(
                        &mut surface,
                        inspector.x.saturating_add(inset),
                        inspector.y.saturating_add((31.0 * scale) as u32),
                        detail,
                        t.muted,
                        scaled_host_ui_font(12, self.session_seed.font_size_logical(), scale),
                        inspector.width.saturating_sub(inset.saturating_mul(2)),
                    );
                }
                if live {
                    let focus = layout.home_focus_action(width, height, scale);
                    surface.fill_rect(
                        focus.x,
                        focus.y,
                        focus.width,
                        focus.height,
                        t.surface.to_xrgb(),
                    );
                    stroke_rect(&mut surface, focus, scale.max(1.0) as u32, t.accent);
                    let label = match self.ui_language {
                        ui::UiLanguage::English => "Focus tab",
                        ui::UiLanguage::ChineseSimplified => "打开标签页",
                        ui::UiLanguage::ChineseTraditional => "開啟分頁",
                    };
                    paint_host_ui_text(
                        &mut surface,
                        focus.x.saturating_add((10.0 * scale) as u32),
                        focus.y.saturating_add((5.0 * scale) as u32),
                        label,
                        t.text,
                        scaled_host_ui_font(13, self.session_seed.font_size_logical(), scale),
                        focus.width.saturating_sub((20.0 * scale) as u32),
                    );
                }
            }
        }
    }

    pub(crate) fn paint_empty_workspace(
        &self,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        scale: f64,
    ) {
        let mut surface = Surface::with_clip(
            pixels,
            width,
            height,
            PixelRect::from_xywh(0, 0, width, height),
        );
        let layout = self.layout(width, height, scale);
        let t = theme::Theme::for_choice(self.ui_theme);
        let tree_bg = t.sidebar_bg;
        let canvas = t.canvas_bg;
        let rule = t.border;
        let muted = t.muted;
        let text = t.text;
        surface.fill_rect(0, 0, width, height, canvas.to_xrgb());
        surface.fill_rect(0, 0, layout.sidebar.width, height, tree_bg.to_xrgb());
        surface.fill_rect(
            layout.sidebar.width.saturating_sub(1),
            0,
            1,
            height,
            rule.to_xrgb(),
        );

        let host_ui_size = |nominal| {
            scaled_host_ui_font(
                nominal,
                self.session_seed.font_size_logical(),
                scale.max(1.0),
            )
        };
        let icon_size = host_ui_size(HOST_UI_HEADER_SIZE_PX);
        self.paint_harness_entry(&mut surface, layout, scale, icon_size);
        for (button, icon, selected) in [
            (
                layout.new_root,
                HeaderIcon::NewRoot,
                self.hovered_header == Some(HeaderIcon::NewRoot),
            ),
            (
                layout.new_web,
                HeaderIcon::NewWeb,
                self.hovered_header == Some(HeaderIcon::NewWeb),
            ),
        ] {
            paint_header_icon_button(
                &mut surface,
                button,
                icon,
                t.accent,
                selected,
                t.surface,
                icon_size,
                scale,
            );
        }
        paint_header_icon_button(
            &mut surface,
            layout.settings,
            HeaderIcon::Settings,
            text,
            self.settings_open || self.hovered_header == Some(HeaderIcon::Settings),
            t.surface,
            icon_size,
            scale,
        );

        let home = layout.home;
        surface.fill_rect(
            home.x,
            home.y,
            home.width,
            home.height,
            t.sidebar_bg.to_xrgb(),
        );
        paint_host_ui_text(
            &mut surface,
            home.x.saturating_add((12.0 * scale) as u32),
            home.y.saturating_add((8.0 * scale) as u32),
            if layout.sidebar_collapsed {
                "@0"
            } else {
                match self.ui_language {
                    ui::UiLanguage::English => "@0  Home",
                    ui::UiLanguage::ChineseSimplified => "@0  首页",
                    ui::UiLanguage::ChineseTraditional => "@0  首頁",
                }
            },
            t.text,
            host_ui_size(HOST_UI_TAB_SIZE_PX),
            home.width.saturating_sub((18.0 * scale) as u32),
        );

        let strings = self.ui_language.strings();
        let button = layout.empty_new_terminal(width, height, scale);
        let title_size = host_ui_size(18);
        let title_metrics = font::cell_metrics(title_size);
        let title_width = title_metrics.width.max(1).saturating_mul(
            u32::try_from(composer::cells(strings.empty_title)).unwrap_or(u32::MAX),
        );
        let content_width = width.saturating_sub(layout.sidebar.width);
        let title_x = layout
            .sidebar
            .width
            .saturating_add(content_width.saturating_sub(title_width) / 2);
        let title_y = button
            .y
            .saturating_sub(title_metrics.height.saturating_add(28));
        paint_host_ui_text(
            &mut surface,
            title_x,
            title_y,
            strings.empty_title,
            muted,
            title_size,
            content_width,
        );
        surface.fill_rect(
            button.x,
            button.y,
            button.width,
            button.height,
            t.surface.to_xrgb(),
        );
        stroke_rect(&mut surface, button, scale.max(1.0) as u32, rule);
        paint_button_label(
            &mut surface,
            button,
            strings.new_terminal,
            text,
            host_ui_size(BUTTON_LABEL_SIZE_PX),
        );
        let hint_size = host_ui_size(13);
        let hint_metrics = font::cell_metrics(hint_size);
        let hint_width = hint_metrics.width.max(1).saturating_mul(
            u32::try_from(composer::cells(strings.new_terminal_hint)).unwrap_or(u32::MAX),
        );
        paint_host_ui_text(
            &mut surface,
            layout
                .sidebar
                .width
                .saturating_add(content_width.saturating_sub(hint_width) / 2),
            button.y.saturating_add(button.height).saturating_add(14),
            strings.new_terminal_hint,
            muted,
            hint_size,
            content_width,
        );
        // With no tabs there is otherwise no way out of the window at all, so
        // the greeting page carries an explicit exit. Drawn muted: quitting is
        // the secondary action next to opening a terminal.
        let quit = layout.empty_quit(width, height, scale);
        surface.fill_rect(quit.x, quit.y, quit.width, quit.height, t.surface.to_xrgb());
        stroke_rect(&mut surface, quit, scale.max(1.0) as u32, rule);
        paint_button_label(
            &mut surface,
            quit,
            strings.quit,
            muted,
            host_ui_size(BUTTON_LABEL_SIZE_PX),
        );
        let clipboard = self.clipboard_readout(layout.status.width);
        paint_status_bar(
            &mut surface,
            layout,
            t,
            "",
            clipboard.as_deref(),
            None,
            host_ui_size(HOST_UI_STATUS_SIZE_PX),
            scale,
        );
        if self.settings_open {
            let restart_cost = self.ui_language.restart_cost(self.workspace.nodes().len());
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                self.font_zoom_percent(),
                &self.update,
                &restart_cost,
                self.settings_section,
                &self.settings_proxy_input,
                self.settings_proxy_focused,
                self.settings_proxy_error.as_deref(),
                &self.config.browser_proxy_options,
                self.settings_proxy_list_start,
            );
            self.paint_harness_settings(
                &mut surface,
                layout,
                scale,
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
            );
        }
        self.paint_new_tab_menu(
            &mut surface,
            layout,
            scale,
            self.session_seed.font_size_logical(),
        );
    }
}

/// The name line of a tab-tree row.
///
/// Precedence: a user-set custom name wins outright; otherwise the program the
/// session runs (`cmd`, `pwsh`, ...); otherwise the node's creation-time label.
/// Deliberately **not** the session's OSC title — that is the separate second
/// line (see [`tree_row_title`]); putting it here too would make the two lines
/// identical and would let a shell's title rewrite rewrite the tab's name.
pub(crate) fn tree_row_name<'a>(
    custom: Option<&'a str>,
    program: Option<&'a str>,
    node_title: &'a str,
) -> &'a str {
    custom
        .or_else(|| program.filter(|name| !name.is_empty()))
        .unwrap_or(node_title)
}

/// The title line of a tab-tree row: the terminal's OSC title, if it has one.
///
/// `None` when the session is gone or the shell has not set a title, so the row
/// simply omits the second line rather than showing a blank one.
pub(crate) fn tree_row_title(session_title: Option<&str>) -> Option<&str> {
    session_title.filter(|title| !title.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{tree_row_name, tree_row_title};

    #[test]
    fn a_custom_name_overrides_the_program_label() {
        assert_eq!(
            tree_row_name(Some("deploy"), Some("pwsh"), "terminal"),
            "deploy"
        );
    }

    #[test]
    fn without_a_custom_name_the_program_label_is_used() {
        assert_eq!(tree_row_name(None, Some("pwsh"), "terminal"), "pwsh");
    }

    #[test]
    fn an_empty_program_label_falls_back_to_the_node_label() {
        assert_eq!(tree_row_name(None, Some(""), "terminal"), "terminal");
        assert_eq!(tree_row_name(None, None, "terminal"), "terminal");
    }

    #[test]
    fn the_title_line_is_the_session_title_or_absent() {
        assert_eq!(tree_row_title(Some("my session")), Some("my session"));
        assert_eq!(tree_row_title(Some("")), None);
        assert_eq!(tree_row_title(None), None);
    }
}
