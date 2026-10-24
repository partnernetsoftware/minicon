//! Process-local browser proxy editing. Product owner: PRD 25.01.
use super::*;
use minicon_core::field_editor::{self, Command, Intent, Limits};

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
        self.browser_chrome_trigger = None;
        self.browser_proxy_focused = true;
        self.browser_address_focused = false;
        if let Some(editor) = &mut self.browser_address {
            field_editor::apply(editor, Command::Blur, Limits::ADDRESS);
        }
        self.settings_proxy_focused = false;
        field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
        self.browser_address_drag = None;
        self.browser_address_clicks = Default::default();
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

    pub(super) fn update_proxy_ime_anchor(&self, window: &PixelWindow) {
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
            field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
            return Ok(false);
        }
        if matches!(
            event,
            PixelWindowEvent::PointerCaptureLost | PixelWindowEvent::FocusChanged(false)
        ) && self.browser_proxy_focused
        {
            self.browser_address_drag = None;
            field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
        }
        if let PixelWindowEvent::PointerMoved { position, .. } = event
            && self.browser_proxy_focused
            && let Some(anchor) = self.browser_address_drag
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let rect = self
                .layout(metrics.physical_width, metrics.physical_height, scale)
                .browser_proxy_text(scale);
            let size = scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
            let offset = super::browser_ui::host_field_offset(
                &self.browser_proxy_input,
                rect,
                position,
                scale,
                size,
            );
            field_editor::drag(&mut self.browser_proxy_input, anchor, offset);
            self.update_proxy_ime_anchor(window);
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Released,
            button: PointerButton::Left,
            ..
        } = event
            && self.browser_proxy_focused
            && self.browser_address_drag.take().is_some()
        {
            let _ = window.set_pointer_capture(false);
            return Ok(true);
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
                let size = scaled_host_ui_font(16, self.session_seed.font_size_logical(), scale);
                let offset = super::browser_ui::host_field_offset(
                    &self.browser_proxy_input,
                    rect,
                    position,
                    scale,
                    size,
                );
                if was_focused {
                    let clicks = self.browser_address_clicks.register(offset, Instant::now());
                    self.browser_address_drag = field_editor::press(
                        &mut self.browser_proxy_input,
                        offset,
                        modifiers.shift,
                        clicks,
                    );
                    let _ = window.set_pointer_capture(self.browser_address_drag.is_some());
                } else {
                    self.focus_browser_proxy(window);
                    self.browser_address_clicks.register(offset, Instant::now());
                }
                self.update_proxy_ime_anchor(window);
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            if self.browser_proxy_focused {
                self.browser_address_drag = None;
                field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
            }
            self.browser_proxy_focused = false;
        }
        if !self.browser_proxy_focused {
            return Ok(false);
        }
        if let PixelWindowEvent::Keyboard(key) = event
            && key.state == KeyPressState::Pressed
            && (key.modifiers.meta || key.modifiers.control)
            && matches!(&key.logical, LogicalKey::Character(text) if text.eq_ignore_ascii_case("l"))
        {
            self.browser_proxy_focused = false;
            field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
            return Ok(false);
        }
        let Some(outcome) =
            super::browser_ui::edit_host_field(&mut self.browser_proxy_input, event, Limits::PROXY)
        else {
            return Ok(false);
        };
        match outcome.intent {
            Intent::Submit => {
                self.apply_browser_proxy(window);
                return Ok(true);
            }
            Intent::Cancel => {
                self.browser_proxy_input = composer::ComposerState::default();
                self.browser_proxy_input.text = self.browser_proxy_applied.clone();
                self.browser_proxy_input.caret = self.browser_proxy_input.text.len();
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
            Intent::None => {}
        }
        self.update_proxy_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }
}
