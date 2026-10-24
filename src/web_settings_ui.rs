//! Web settings and saved proxy choices. Product owner: PRD 25.01.
use super::*;
use minicon_core::field_editor::{self, Command, Intent, Limits};

impl ConApp {
    fn proxy_option_error(&self, error: web_preferences::ProxyOptionError) -> String {
        use web_preferences::ProxyOptionError::*;
        match (error, self.ui_language) {
            (Invalid, ui::UiLanguage::English) => {
                "Enter an HTTP proxy such as http://127.0.0.1:8890."
            }
            (Duplicate, ui::UiLanguage::English) => "This proxy is already in the list.",
            (Full, ui::UiLanguage::English) => "You can save up to eight proxy choices.",
            (Invalid, ui::UiLanguage::ChineseSimplified) => {
                "请输入 HTTP 代理，例如 http://127.0.0.1:8890。"
            }
            (Duplicate, ui::UiLanguage::ChineseSimplified) => "该代理已在列表中。",
            (Full, ui::UiLanguage::ChineseSimplified) => "最多可保存 8 个代理选项。",
            (Invalid, ui::UiLanguage::ChineseTraditional) => {
                "請輸入 HTTP 代理，例如 http://127.0.0.1:8890。"
            }
            (Duplicate, ui::UiLanguage::ChineseTraditional) => "此代理已在清單中。",
            (Full, ui::UiLanguage::ChineseTraditional) => "最多可儲存 8 個代理選項。",
        }
        .to_owned()
    }

    fn save_proxy_options(&mut self, previous: Vec<String>) -> bool {
        let saved = config_path()
            .ok_or_else(|| "configuration directory unavailable".to_owned())
            .and_then(|path| write_config_to(&path, &self.config));
        if saved.is_err() {
            self.config.browser_proxy_options = previous;
            self.settings_proxy_error = Some(
                match self.ui_language {
                    ui::UiLanguage::English => {
                        "Could not save choices. The previous list is retained."
                    }
                    ui::UiLanguage::ChineseSimplified => "无法保存候选项，原有列表已保留。",
                    ui::UiLanguage::ChineseTraditional => "無法儲存候選項，原有清單已保留。",
                }
                .to_owned(),
            );
            return false;
        }
        self.settings_proxy_error = None;
        true
    }

    fn add_settings_proxy(&mut self, window: &PixelWindow) {
        let previous = self.config.browser_proxy_options.clone();
        match web_preferences::add_proxy_option(
            &mut self.config.browser_proxy_options,
            &self.settings_proxy_input.text,
        ) {
            Ok(()) => {
                if self.save_proxy_options(previous) {
                    self.settings_proxy_input = composer::ComposerState::default();
                }
            }
            Err(error) => self.settings_proxy_error = Some(self.proxy_option_error(error)),
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn focus_settings_proxy(&mut self, window: &PixelWindow) {
        self.browser_chrome_trigger = None;
        self.settings_proxy_focused = true;
        self.browser_proxy_focused = false;
        self.browser_address_focused = false;
        field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
        if let Some(editor) = &mut self.browser_address {
            field_editor::apply(editor, Command::Blur, Limits::ADDRESS);
        }
        self.browser_address_drag = None;
        self.browser_address_clicks = Default::default();
        self.composer.focused = false;
        composer::select_all(&mut self.settings_proxy_input);
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            page.focus_parent();
        }
        #[cfg(target_os = "linux")]
        window.focus();
        self.update_settings_proxy_anchor(window);
    }

    pub(super) fn update_settings_proxy_anchor(&self, window: &PixelWindow) {
        let Ok(metrics) = window.metrics() else {
            return;
        };
        let scale = metrics.scale_factor.max(1.0);
        let rect = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .web_settings(scale, self.settings_proxy_list_start)
            .proxy_input;
        let size = scaled_host_ui_font(HOST_UI_STATUS_SIZE_PX, self.host_font_logical(), scale);
        let cell = font::cell_metrics(size).width.max(1);
        let editor = &self.settings_proxy_input;
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

    pub(super) fn route_web_settings(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if !self.settings_open {
            if self.settings_proxy_focused {
                self.browser_address_drag = None;
            }
            self.settings_proxy_focused = false;
            field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
            return Ok(false);
        }
        if matches!(
            event,
            PixelWindowEvent::PointerCaptureLost | PixelWindowEvent::FocusChanged(false)
        ) && self.settings_proxy_focused
        {
            self.browser_address_drag = None;
            field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
        }
        if let PixelWindowEvent::PointerMoved { position, .. } = event
            && self.settings_proxy_focused
            && let Some(anchor) = self.browser_address_drag
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let rect = self
                .layout(metrics.physical_width, metrics.physical_height, scale)
                .web_settings(scale, self.settings_proxy_list_start)
                .proxy_input;
            let size = scaled_host_ui_font(HOST_UI_STATUS_SIZE_PX, self.host_font_logical(), scale);
            let offset = super::browser_ui::host_field_offset(
                &self.settings_proxy_input,
                rect,
                position,
                scale,
                size,
            );
            field_editor::drag(&mut self.settings_proxy_input, anchor, offset);
            self.update_settings_proxy_anchor(window);
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            state: PointerButtonState::Released,
            button: PointerButton::Left,
            ..
        } = event
            && self.settings_proxy_focused
            && self.browser_address_drag.take().is_some()
        {
            let _ = window.set_pointer_capture(false);
            return Ok(true);
        }
        if let PixelWindowEvent::PointerButton {
            button: PointerButton::Left,
            state: PointerButtonState::Pressed,
            position: Some(position),
            modifiers,
        } = event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
            let web = layout.web_settings(scale, self.settings_proxy_list_start);
            let routes = if self.settings_section != ui::SettingsSection::Console {
                web.routes
            } else {
                layout.settings_panel(scale).routes
            };
            let x = (position.x * scale).max(0.0) as u32;
            let y = (position.y * scale).max(0.0) as u32;
            for (index, rect) in routes.iter().enumerate() {
                if rect.contains(x, y) {
                    self.settings_section = ui::SettingsSection::from_index(index);
                    self.blur_harness_fields();
                    if self.settings_section == ui::SettingsSection::Harness {
                        self.poll_harness_settings(window);
                    }
                    self.settings_proxy_focused = false;
                    self.browser_address_drag = None;
                    field_editor::apply(
                        &mut self.settings_proxy_input,
                        Command::Blur,
                        Limits::PROXY,
                    );
                    self.browser_proxy_focused = false;
                    self.browser_proxy_menu_open = false;
                    self.settings_proxy_list_start = 0;
                    self.sync_embedded_page(window);
                    self.mark_host_ui_full_and_repaint(window);
                    return Ok(true);
                }
            }
            if self.settings_section != ui::SettingsSection::Web {
                return Ok(false);
            }
            if web.proxy_input.contains(x, y) {
                let was_focused = self.settings_proxy_focused;
                let size =
                    scaled_host_ui_font(HOST_UI_STATUS_SIZE_PX, self.host_font_logical(), scale);
                let offset = super::browser_ui::host_field_offset(
                    &self.settings_proxy_input,
                    web.proxy_input,
                    position,
                    scale,
                    size,
                );
                if was_focused {
                    let clicks = self.browser_address_clicks.register(offset, Instant::now());
                    self.browser_address_drag = field_editor::press(
                        &mut self.settings_proxy_input,
                        offset,
                        modifiers.shift,
                        clicks,
                    );
                    let _ = window.set_pointer_capture(self.browser_address_drag.is_some());
                } else {
                    self.focus_settings_proxy(window);
                    self.browser_address_clicks.register(offset, Instant::now());
                }
                self.update_settings_proxy_anchor(window);
            } else if web.proxy_add.contains(x, y) {
                self.add_settings_proxy(window);
            } else {
                self.settings_proxy_focused = false;
                self.browser_address_drag = None;
                field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
                let remove = web
                    .proxy_remove
                    .iter()
                    .take(web.visible_rows)
                    .position(|rect| rect.contains(x, y))
                    .map(|slot| slot + web.list_start);
                if let Some(index) =
                    remove.filter(|&index| index < self.config.browser_proxy_options.len())
                {
                    let previous = self.config.browser_proxy_options.clone();
                    self.config.browser_proxy_options.remove(index);
                    self.save_proxy_options(previous);
                    self.settings_proxy_list_start = self.settings_proxy_list_start.min(
                        self.config
                            .browser_proxy_options
                            .len()
                            .saturating_sub(web.visible_rows),
                    );
                } else if web.previous.contains(x, y) {
                    self.settings_proxy_list_start = self
                        .settings_proxy_list_start
                        .saturating_sub(web.visible_rows.max(1));
                } else if web.next.contains(x, y) {
                    self.settings_proxy_list_start = self
                        .settings_proxy_list_start
                        .saturating_add(web.visible_rows.max(1))
                        .min(
                            self.config
                                .browser_proxy_options
                                .len()
                                .saturating_sub(web.visible_rows),
                        );
                } else if !web.panel.contains(x, y) {
                    if layout.settings.contains(x, y) || layout.new_root.contains(x, y) {
                        return Ok(false);
                    }
                    self.settings_open = false;
                    self.sync_embedded_page(window);
                    self.mark_host_ui_full_and_repaint(window);
                    return Ok(false);
                }
            }
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if self.settings_section != ui::SettingsSection::Web {
            return Ok(false);
        }
        if let PixelWindowEvent::Keyboard(key) = event
            && !self.settings_proxy_focused
            && key.state == KeyPressState::Pressed
            && matches!(key.logical, LogicalKey::Named(NamedKey::Escape))
        {
            self.settings_proxy_focused = false;
            self.settings_open = false;
            self.sync_embedded_page(window);
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if !self.settings_proxy_focused {
            return Ok(matches!(
                event,
                PixelWindowEvent::Keyboard(_)
                    | PixelWindowEvent::Ime(_)
                    | PixelWindowEvent::MouseWheel { .. }
            ));
        }
        let Some(outcome) = super::browser_ui::edit_host_field(
            &mut self.settings_proxy_input,
            event,
            Limits::PROXY,
        ) else {
            return Ok(false);
        };
        match outcome.intent {
            Intent::Submit => {
                self.add_settings_proxy(window);
                return Ok(true);
            }
            Intent::Cancel => {
                self.settings_proxy_focused = false;
                self.settings_open = false;
                self.browser_address_drag = None;
                field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
                self.sync_embedded_page(window);
            }
            Intent::None => {}
        }
        self.update_settings_proxy_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
        Ok(true)
    }
}
