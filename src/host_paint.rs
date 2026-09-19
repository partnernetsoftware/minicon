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

impl ConApp {
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
        let header_height = layout.tree_header_height;
        let row_height = layout.tree_row_height;
        let host_ui_size = |nominal| scaled_host_ui_font(nominal, session.font_size_logical, scale);
        // The host UI (sidebar, tabs, composer, borders) is themed; the
        // terminal body keeps its ANSI/xterm colors regardless. Only these
        // outer surfaces change when the user switches themes.
        let t = theme::Theme::for_choice(self.ui_theme);
        let tree_bg = t.sidebar_bg;
        let tree_rule = t.border;
        let branch = t.branch;
        let active_bg = t.surface;
        let accent = t.accent;
        let error_accent = t.error;
        let composer_bg = t.composer_bg;
        let text = t.text;
        let muted = t.muted;
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
            width.saturating_sub(tree_width),
            session.content_bottom_px,
            composer_bg.to_xrgb(),
        );
        surface.fill_rect(
            tree_width,
            height.saturating_sub(session.content_bottom_px),
            width.saturating_sub(tree_width),
            1,
            tree_rule.to_xrgb(),
        );

        let header_icon_size = host_ui_size(HOST_UI_HEADER_SIZE_PX);
        paint_header_icon_button(
            &mut surface,
            layout.new_root,
            HeaderIcon::NewRoot,
            accent,
            self.hovered_header == Some(HeaderIcon::NewRoot),
            t.surface,
            header_icon_size,
            scale,
        );
        paint_header_icon_button(
            &mut surface,
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
            &mut surface,
            layout.sidebar_toggle,
            toggle_icon,
            accent,
            self.hovered_header == Some(toggle_icon),
            t.surface,
            header_icon_size,
            scale,
        );
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
            // `ListTabs` reports `current_title` directly; this row is the third
            // surface that shows a title, so it filters empty and falls back to
            // the node's own label. `current_title` is never empty once a
            // session exists (`session_label` always yields a non-empty name),
            // so the two agree today — the fallback only guards the tree against
            // a future session that reports one.
            let title = session
                .map(|terminal| terminal.current_title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or(node.title.as_str());
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
                let text_width = host_ui_text_width(&label, host_ui_size(HOST_UI_TAB_SIZE_PX));
                let x = tree_width.saturating_sub(text_width) / 2;
                paint_host_ui_text(
                    &mut surface,
                    x,
                    y + 7,
                    &label,
                    label_color,
                    host_ui_size(HOST_UI_TAB_SIZE_PX),
                    tree_width,
                );
                if row_hovered {
                    rail_tooltip = Some((visible_index as u32, title.to_owned()));
                }
                continue;
            }
            paint_host_ui_text_parts_clipped(
                &mut surface,
                indent,
                y + 7,
                &["@", id.format(node.id.get()), "  ", title],
                label_color,
                host_ui_size(HOST_UI_TAB_SIZE_PX),
                tree_width.saturating_sub(indent + 38),
            );
            if row_active || row_hovered {
                let close = layout.tree_close_rect(visible_index, scale);
                // A hover plate behind the glyph so it reads as a button, not
                // floating text.
                if row_hovered {
                    surface.fill_rect(
                        close.x,
                        close.y,
                        close.width,
                        close.height,
                        active_bg.to_xrgb(),
                    );
                }
                paint_host_ui_text(
                    &mut surface,
                    close.x + 6,
                    close.y + 3,
                    "x",
                    text,
                    host_ui_size(HOST_UI_CLOSE_SIZE_PX),
                    close.width.saturating_sub(6),
                );
            }
        }

        if let Some((visible_row, title)) = rail_tooltip {
            let tip_size = host_ui_size(HOST_UI_TAB_SIZE_PX);
            let text_width = host_ui_text_width(&title, tip_size);
            if let Some(tip) = ui::rail_tooltip_rect(layout, visible_row, text_width, width, scale)
            {
                surface.fill_rect(tip.x, tip.y, tip.width, tip.height, active_bg.to_xrgb());
                stroke_rect(&mut surface, tip, 1, tree_rule);
                let padding = tip.width.saturating_sub(text_width) / 2;
                paint_host_ui_text(
                    &mut surface,
                    tip.x.saturating_add(padding),
                    tip.y.saturating_add(tip.height / 2).saturating_sub(7),
                    &title,
                    text,
                    tip_size,
                    tip.width.saturating_sub(padding),
                );
            }
        }

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
            paint_host_ui_text_parts(
                &mut surface,
                header_x,
                input_y + 7,
                // Names the tab, not just its number. The tab column and the
                // window title both call it by name now, and the input area saying
                // where text is going is the whole reason this band is worth its
                // permanent share of the window — "@1" is only an answer if you
                // already know what @1 is.
                &[
                    self.ui_language.strings().send_to,
                    active_id_text.format(active_id),
                    " ",
                    self.active_session_opt()
                        .map_or("", |session| session.current_title.as_str()),
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
            .composer_input
            .width
            .saturating_sub(COMPOSER_TEXT_INSET.saturating_mul(2));
        let composer_cells = (composer_text_width / composer_cell_width) as usize;
        let rows =
            (layout.composer_input.height.saturating_sub(8) / composer_line_height).max(1) as usize;
        let lines = composer::visible_line_window(&self.composer.text, self.composer.caret, rows);
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
        paint_status_bar(
            &mut surface,
            layout,
            t,
            status_label,
            status_cursor,
            host_ui_size(HOST_UI_STATUS_SIZE_PX),
            scale,
        );
        if self.settings_open {
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                self.active_session_opt().map_or(100, |session| {
                    ((session.font_size_logical / session.font_size_baseline.max(1.0)) * 100.0)
                        .round() as u16
                }),
            );
        }
        Ok(())
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
            scaled_host_ui_font(nominal, self.session_seed.font_size_logical, scale.max(1.0))
        };
        let icon_size = host_ui_size(HOST_UI_HEADER_SIZE_PX);
        for (button, icon, selected) in [
            (
                layout.new_root,
                HeaderIcon::NewRoot,
                self.hovered_header == Some(HeaderIcon::NewRoot),
            ),
            (
                layout.settings,
                HeaderIcon::Settings,
                self.settings_open || self.hovered_header == Some(HeaderIcon::Settings),
            ),
        ] {
            paint_header_icon_button(
                &mut surface,
                button,
                icon,
                text,
                selected,
                t.surface,
                icon_size,
                scale,
            );
        }

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
        paint_status_bar(
            &mut surface,
            layout,
            t,
            "",
            None,
            host_ui_size(HOST_UI_STATUS_SIZE_PX),
            scale,
        );
        if self.settings_open {
            paint_settings_panel(
                &mut surface,
                layout,
                scale,
                t,
                self.ui_language,
                self.ui_theme,
                self.ui_language.help_lines(),
                host_ui_size(HOST_UI_STATUS_SIZE_PX),
                self.active_session_opt().map_or(100, |session| {
                    ((session.font_size_logical / session.font_size_baseline.max(1.0)) * 100.0)
                        .round() as u16
                }),
            );
        }
    }
}
