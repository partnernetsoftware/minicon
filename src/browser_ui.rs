//! Browser workspace chrome and address editing. Product behavior: PRD 25.
use super::*;

impl ConApp {
    pub(super) fn focus_browser_address(&mut self, window: &PixelWindow) {
        let mut editor = composer::ComposerState::default();
        editor.text = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get(&id))
            .and_then(|tab| tab.panel().url.clone())
            .unwrap_or_default();
        editor.caret = editor.text.len();
        composer::select_all(&mut editor);
        self.browser_address = Some(editor);
        self.composer.focused = false;
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            page.focus_parent();
        }
        #[cfg(target_os = "linux")]
        window.focus();
        self.update_browser_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn route_browser_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if !self.active_is_browser() {
            self.browser_address = None;
            return Ok(false);
        }
        if let PixelWindowEvent::Keyboard(key) = event
            && key.state == KeyPressState::Pressed
            && (key.modifiers.control || key.modifiers.meta)
            && matches!(&key.logical, LogicalKey::Character(text) if text.eq_ignore_ascii_case("l"))
        {
            self.focus_browser_address(window);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Pressed,
            button: PointerButton::Left,
            position: Some(position),
            ..
        } = event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
            let x = (position.x * scale).max(0.0) as u32;
            let y = (position.y * scale).max(0.0) as u32;
            if layout.browser_toolbar(scale).contains(x, y) {
                if layout.browser_address(scale).contains(x, y) {
                    self.focus_browser_address(window);
                } else {
                    let control =
                        ((x.saturating_sub(layout.composer.x)) as f64 / (34.0 * scale)) as u32;
                    self.browser_navigation(window, control);
                }
                return Ok(true);
            }
            self.browser_address = None;
        }
        let Some(editor) = self.browser_address.as_mut() else {
            return Ok(false);
        };
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
                    keymap::Action::SoftNewline | keymap::Action::Send => {
                        let text = editor.text.trim().to_owned();
                        let url = if text.contains("://") {
                            text
                        } else {
                            format!("https://{text}")
                        };
                        if browser::http_url_at(&url, 0).as_deref() == Some(url.as_str()) {
                            self.browser_address = None;
                            self.present_http_link(window, &url);
                            self.sync_embedded_page(window);
                            #[cfg(any(target_os = "macos", windows))]
                            if let Some(page) = &self.native_browser_page {
                                page.focus();
                            }
                            #[cfg(target_os = "linux")]
                            if let Some(process) = &self.browser_process {
                                let _ = process.send("focus", None);
                            }
                        }
                    }
                    keymap::Action::CancelFocus => {
                        self.browser_address = None;
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
                            if editor.text.len() + text.len() <= 8192 {
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
                            && editor.text.len() + text.len() <= 8192
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
                        if editor.text.len() + text.len() <= 8192 {
                            composer::insert(editor, &single_line_label(&text));
                        }
                    }
                    _ => {}
                }
            }
            _ => return Ok(false),
        }
        self.update_browser_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }

    fn update_browser_ime_anchor(&self, window: &PixelWindow) {
        let Some(editor) = &self.browser_address else {
            return;
        };
        let Ok(metrics) = window.metrics() else {
            return;
        };
        let scale = metrics.scale_factor.max(1.0);
        let rect = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .browser_address(scale);
        let size = scaled_host_ui_font(14, self.session_seed.font_size_logical(), scale);
        let cell = font::cell_metrics(size).width.max(1);
        let cells = (rect.width.saturating_sub((16.0 * scale) as u32) / cell).max(1) as usize;
        let visible =
            composer::visible_window(&editor.text, &editor.preedit, editor.caret, 1, cells);
        let caret = editor.caret.clamp(visible.text, editor.text.len());
        let offset = composer::cells(&editor.text[visible.text..caret])
            + composer::cells(&editor.preedit[visible.preedit..]);
        let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
            f64::from(rect.x + cell.saturating_mul(offset as u32)) / scale + 8.0,
            f64::from(rect.y) / scale + 7.0,
            2.0,
            20.0,
        ));
    }

    fn browser_navigation(&mut self, window: &PixelWindow, control: u32) {
        if control > 2
            || (control == 0 && !self.browser_can_back)
            || (control == 1 && !self.browser_can_forward)
        {
            return;
        }
        self.browser_address = None;
        self.browser_load_deadline = Some(Instant::now() + Duration::from_secs(30));
        #[cfg(target_os = "linux")]
        if let Some(process) = &self.browser_process {
            let action = match control {
                0 if self.browser_can_back => "back",
                1 if self.browser_can_forward => "forward",
                2 => "reload",
                _ => return,
            };
            if let Err(reason) = process.send(action, None) {
                self.note_page_load(browser::PageLoad::Unavailable {
                    url: String::new(),
                    reason,
                });
            }
        }
        if control == 2 && self.browser_page.is_none() {
            #[cfg(any(target_os = "macos", windows))]
            let missing = self.native_browser_page.is_none();
            #[cfg(not(any(target_os = "macos", windows)))]
            let missing = true;
            if missing {
                let url = self
                    .workspace
                    .browser_tab()
                    .and_then(|id| self.browsers.get(&id))
                    .and_then(|tab| tab.panel().url.clone());
                if let Some(url) = url {
                    self.launch_page_host(window, &url);
                }
            }
        }
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            let result = match control {
                0 if self.browser_can_back => page.back(),
                1 if self.browser_can_forward => page.forward(),
                2 => page.reload(),
                _ => return,
            };
            if let Err(reason) = result {
                let url = self
                    .workspace
                    .browser_tab()
                    .and_then(|id| self.browsers.get(&id))
                    .and_then(|tab| tab.panel().url.clone())
                    .unwrap_or_default();
                self.note_page_load(browser::PageLoad::Unavailable { url, reason });
            }
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(super) fn drain_browser_state(&mut self, window: &PixelWindow) {
        let Some(state) = self
            .native_browser_page
            .as_ref()
            .and_then(|page| page.take_state())
        else {
            return;
        };
        self.browser_can_back = state.can_back;
        self.browser_can_forward = state.can_forward;
        self.browser_load_deadline = if state.loading {
            self.browser_load_deadline
                .or(Some(Instant::now() + Duration::from_secs(30)))
        } else {
            None
        };
        for shortcut in state.shortcuts {
            if shortcut == 'L' {
                self.focus_browser_address(window);
            } else {
                let key = injected_key_event(
                    InjectedKey::Char(shortcut.to_ascii_lowercase()),
                    true,
                    false,
                    true,
                );
                let _ = self.handle_workspace_shortcut(window, &key);
            }
        }
        if let Some(id) = self.workspace.browser_tab()
            && let Some(tab) = self.browsers.get_mut(&id)
        {
            tab.note_title(&state.title);
            if let Some(reason) = state.error {
                let url = tab.panel().url.clone().unwrap_or(state.url);
                tab.note_load(browser::PageLoad::Unavailable { url, reason });
            } else if browser::http_url_at(&state.url, 0).as_deref() == Some(state.url.as_str()) {
                tab.note_load(if state.loading {
                    browser::PageLoad::Loading { url: state.url }
                } else {
                    browser::PageLoad::Ready { url: state.url }
                });
            }
        }
        if let Some(page) = &self.native_browser_page {
            let _ = page.set_visible(self.browser_page_visible());
        }
        if self.active_is_browser() {
            let _ = self.refresh_title(window);
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    #[cfg(target_os = "linux")]
    pub(super) fn drain_browser_process(&mut self, window: &PixelWindow) {
        let state = self
            .browser_process
            .as_ref()
            .and_then(|process| process.take_state());
        if let Some(state) = state {
            if let Some(keys) = state["shortcuts"].as_array() {
                for key in keys
                    .iter()
                    .filter_map(|key| key.as_str().and_then(|s| s.chars().next()))
                {
                    if key == 'L' {
                        self.focus_browser_address(window);
                    } else {
                        let key = injected_key_event(
                            InjectedKey::Char(key.to_ascii_lowercase()),
                            true,
                            false,
                            true,
                        );
                        let _ = self.handle_workspace_shortcut(window, &key);
                    }
                }
            }
            if let Some(back) = state["can_back"].as_bool() {
                self.browser_can_back = back;
            }
            if let Some(forward) = state["can_forward"].as_bool() {
                self.browser_can_forward = forward;
            }
            let loading = state["loading"].as_bool().unwrap_or(false);
            if state.get("loading").is_some() {
                self.browser_load_deadline = if loading {
                    self.browser_load_deadline
                        .or(Some(Instant::now() + Duration::from_secs(30)))
                } else {
                    None
                };
            }
            if let Some(id) = self.workspace.browser_tab()
                && let Some(tab) = self.browsers.get_mut(&id)
            {
                if let Some(title) = state["title"].as_str() {
                    tab.note_title(title);
                }
                let url = state["url"].as_str().unwrap_or("");
                if let Some(reason) = state["error"].as_str() {
                    tab.note_load(browser::PageLoad::Unavailable {
                        url: tab.panel().url.clone().unwrap_or_default(),
                        reason: reason.to_owned(),
                    });
                } else if browser::http_url_at(url, 0).as_deref() == Some(url) {
                    tab.note_load(if loading {
                        browser::PageLoad::Loading {
                            url: url.to_owned(),
                        }
                    } else {
                        browser::PageLoad::Ready {
                            url: url.to_owned(),
                        }
                    });
                }
            }
            if self.active_is_browser() {
                let _ = self.refresh_title(window);
            }
            // The helper is born asynchronously. A tab/settings/geometry
            // change may predate its X11 child, so reapply the current bounds
            // and visibility once its first native state arrives.
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
        }
        if let Some(child) = &mut self.browser_page
            && let Ok(Some(status)) = child.try_wait()
        {
            let url = self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .and_then(|tab| tab.panel().url.clone())
                .unwrap_or_default();
            self.stop_page_host();
            self.note_page_load(browser::page_load_from_exit(&url, status.code()));
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    pub(super) fn expire_browser_load(&mut self, window: &PixelWindow, now: Instant) {
        if self
            .browser_load_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.browser_load_deadline = None;
            let url = self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .and_then(|tab| tab.panel().url.clone())
                .unwrap_or_default();
            if let Some(id) = self.workspace.browser_tab()
                && let Some(tab) = self.browsers.get_mut(&id)
            {
                tab.note_load(browser::PageLoad::Unavailable {
                    url,
                    reason: "browser-navigation-timeout".to_owned(),
                });
            }
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
        }
    }
}
