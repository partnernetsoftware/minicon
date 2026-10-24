//! Process-local browser proxy editing. Product owner: PRD 25.
use super::*;

impl ConApp {
    pub(super) fn effective_browser_proxy(&self) -> Result<browser::BrowserProxy, String> {
        minicon_web::proxy_override(&self.browser_proxy(), &self.browser_proxy_applied)
    }

    pub(super) fn apply_browser_proxy(&mut self, window: &PixelWindow) {
        self.browser_proxy_applied = self.browser_proxy_input.text.trim().to_owned();
        self.browser_proxy_input.text = self.browser_proxy_applied.clone();
        self.browser_proxy_input.caret = self.browser_proxy_input.text.len();
        self.browser_proxy_input.anchor = None;
        self.browser_proxy_input.select_all = false;
        self.browser_proxy_input.preedit.clear();
        self.browser_proxy_focused = false;
        let url = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get(&id))
            .and_then(|tab| tab.panel().url.clone())
            .unwrap_or_default();
        // Proxy options are creation-time settings. Drop the old connection
        // before validation so an invalid endpoint cannot retain a live route.
        self.stop_page_host();
        if let Some(tab) = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get_mut(&id))
        {
            tab.note_title("");
        }
        if let Err(reason) = self.effective_browser_proxy() {
            self.note_page_load(browser::PageLoad::Unavailable { url, reason });
        } else if !url.is_empty() {
            self.launch_page_host(window, &url);
        } else {
            #[cfg(any(target_os = "macos", windows))]
            {
                self.ensure_native_page(window);
                if self.native_browser_page.is_some() {
                    self.note_page_load(browser::PageLoad::Idle);
                }
            }
        }
        self.sync_embedded_page(window);
        let _ = self.refresh_title(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn focus_browser_proxy(&mut self, window: &PixelWindow) {
        self.browser_proxy_focused = true;
        self.browser_address = None;
        self.browser_address_drag = None;
        self.composer.focused = false;
        composer::select_all(&mut self.browser_proxy_input);
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            page.focus_parent();
        }
        #[cfg(target_os = "linux")]
        window.focus();
        self.update_proxy_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    fn update_proxy_ime_anchor(&self, window: &PixelWindow) {
        let Ok(metrics) = window.metrics() else {
            return;
        };
        let scale = metrics.scale_factor.max(1.0);
        let rect = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .browser_proxy_text(scale);
        let size = scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
        let cell = font::cell_metrics(size).width.max(1);
        let editor = &self.browser_proxy_input;
        let columns = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
        let visible =
            composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, columns);
        let caret = editor.caret.clamp(visible.text, editor.text.len());
        let offset = composer::cells(&editor.text[visible.text..caret])
            + composer::cells(&editor.preedit[visible.preedit..]);
        let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
            f64::from(rect.x + cell.saturating_mul(offset as u32)) / scale + 8.0,
            f64::from(rect.y + rect.height) / scale,
            2.0,
            20.0,
        ));
    }

    pub(super) fn route_browser_proxy_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if !self.active_is_browser() {
            self.browser_proxy_focused = false;
            return Ok(false);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Pressed,
            button: PointerButton::Left,
            position: Some(position),
            modifiers,
        } = event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let rect = self
                .layout(metrics.physical_width, metrics.physical_height, scale)
                .browser_proxy_text(scale);
            if rect.contains(
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            ) {
                let was_focused = self.browser_proxy_focused;
                self.browser_proxy_focused = true;
                self.browser_address = None;
                self.browser_address_drag = None;
                self.composer.focused = false;
                if was_focused {
                    let editor = &mut self.browser_proxy_input;
                    let size =
                        scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
                    let cell = font::cell_metrics(size).width.max(1);
                    let columns =
                        (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
                    let visible = composer::visible_window(
                        &editor.text,
                        &editor.preedit,
                        editor.caret,
                        1,
                        columns,
                    );
                    let column = ((position.x * scale).max(0.0) as u32)
                        .saturating_sub(rect.x + (8.0 * scale) as u32)
                        / cell;
                    let offset =
                        composer::caret_at_cell(&editor.text, visible.text, column as usize);
                    let anchor =
                        editor
                            .anchor
                            .unwrap_or(if editor.select_all { 0 } else { editor.caret });
                    editor.select_all = false;
                    editor.anchor = if modifiers.shift { Some(anchor) } else { None };
                    editor.caret = offset;
                } else {
                    composer::select_all(&mut self.browser_proxy_input);
                }
                #[cfg(any(target_os = "macos", windows))]
                if let Some(page) = &self.native_browser_page {
                    page.focus_parent();
                }
                #[cfg(target_os = "linux")]
                window.focus();
                self.update_proxy_ime_anchor(window);
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            self.browser_proxy_focused = false;
        }
        if !self.browser_proxy_focused {
            return Ok(false);
        }
        if let PixelWindowEvent::Keyboard(key) = event
            && (key.modifiers.meta || key.modifiers.control)
            && matches!(&key.logical, LogicalKey::Character(text) if text.eq_ignore_ascii_case("l"))
        {
            self.browser_proxy_focused = false;
            return Ok(false);
        }
        let editor = &mut self.browser_proxy_input;
        match event {
            PixelWindowEvent::Keyboard(key) => {
                if key.state != KeyPressState::Pressed {
                    return Ok(true);
                }
                let Some(chord) = composer_chord(key) else {
                    return Ok(true);
                };
                let Some(action) =
                    keymap::action(chord, CLIPBOARD_MODIFIER, keymap::Draft::single_line())
                else {
                    return Ok(true);
                };
                match action {
                    keymap::Action::Send | keymap::Action::SoftNewline => {
                        self.apply_browser_proxy(window);
                        return Ok(true);
                    }
                    keymap::Action::CancelFocus => {
                        *editor = composer::ComposerState::default();
                        editor.text = self.browser_proxy_applied.clone();
                        editor.caret = editor.text.len();
                        self.browser_proxy_focused = false;
                        #[cfg(any(target_os = "macos", windows))]
                        if let Some(page) = &self.native_browser_page {
                            page.focus();
                        }
                        #[cfg(target_os = "linux")]
                        if let Some(process) = &self.browser_process {
                            let _ = process.send("focus", None);
                        }
                    }
                    keymap::Action::SelectAll => composer::select_all(editor),
                    keymap::Action::Copy => {
                        if let Some(text) = composer::selection_text(editor) {
                            let _ = clipboard_status::set_text(text);
                        }
                    }
                    keymap::Action::Cut => {
                        if let Some(text) = composer::cut(editor) {
                            let _ = clipboard_status::set_text(&text);
                        }
                    }
                    keymap::Action::Paste => {
                        if let Some(text) = composer_paste_text_or_image_path() {
                            let text = single_line_label(&text);
                            if editor.text.len() + text.len() <= 2048 {
                                composer::insert(editor, &text);
                            }
                        }
                    }
                    keymap::Action::Backspace => composer::backspace(editor),
                    keymap::Action::DeleteForward => composer::delete_forward(editor),
                    keymap::Action::DeleteWordBack => composer::delete_word_back(editor),
                    keymap::Action::DeleteWordForward => composer::delete_word_forward(editor),
                    keymap::Action::Move(movement) => composer::move_caret(editor, movement),
                    keymap::Action::Extend(movement) => {
                        composer::extend_selection(editor, movement)
                    }
                    keymap::Action::Insert => {
                        if let LogicalKey::Character(text) = &key.logical
                            && editor.text.len() + text.len() <= 2048
                        {
                            composer::insert(editor, &single_line_label(text));
                        }
                    }
                    _ => {}
                }
            }
            PixelWindowEvent::Ime(ime) => {
                use minicon_platform::ime::{ImeAction, classify_event};
                match classify_event(ime.clone(), true) {
                    ImeAction::UpdatePreedit { text, .. } => {
                        editor.preedit = text.chars().take(512).collect()
                    }
                    ImeAction::ClearPreedit => editor.preedit.clear(),
                    ImeAction::CommitText(text) => {
                        editor.preedit.clear();
                        if editor.text.len() + text.len() <= 2048 {
                            composer::insert(editor, &single_line_label(&text));
                        }
                    }
                    _ => {}
                }
            }
            _ => return Ok(false),
        }
        self.update_proxy_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }
}
