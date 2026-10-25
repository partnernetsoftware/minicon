//! Browser chrome and bounded menu routing. Product owner: PRD 25.02.
use super::*;
use minicon_core::field_editor::{self, Command, Limits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ChromeTrigger {
    New,
    Proxy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MenuFocus {
    Address,
    Proxy,
    Candidate,
    Settings,
    NewTrigger,
    ProxyTrigger,
    Page,
    Composer,
    Notes,
    Terminal,
}

fn menu_key(key: &NormalizedKeyEvent) -> Option<NamedKey> {
    match &key.logical {
        LogicalKey::Named(named) => Some(*named),
        LogicalKey::Character(text) if text == " " => Some(NamedKey::Space),
        _ => None,
    }
}

impl ConApp {
    pub(super) fn blur_browser_composition(&mut self) {
        self.cancel_harness_pointer();
        if let Some(editor) = &mut self.browser_address {
            field_editor::apply(editor, Command::Blur, Limits::ADDRESS);
        }
        field_editor::apply(&mut self.browser_proxy_input, Command::Blur, Limits::PROXY);
        field_editor::apply(&mut self.settings_proxy_input, Command::Blur, Limits::PROXY);
        self.browser_address_drag = None;
    }

    pub(super) fn current_menu_focus(&self) -> MenuFocus {
        if self.settings_open && self.settings_proxy_focused {
            MenuFocus::Candidate
        } else if self.settings_open {
            MenuFocus::Settings
        } else if self.browser_address_focused {
            MenuFocus::Address
        } else if self.browser_proxy_focused {
            MenuFocus::Proxy
        } else if let Some(trigger) = self.browser_chrome_trigger {
            match trigger {
                ChromeTrigger::New => MenuFocus::NewTrigger,
                ChromeTrigger::Proxy => MenuFocus::ProxyTrigger,
            }
        } else if self.active_is_browser() {
            MenuFocus::Page
        } else if self.notes_focused {
            MenuFocus::Notes
        } else if self.composer.focused {
            MenuFocus::Composer
        } else {
            MenuFocus::Terminal
        }
    }

    pub(super) fn begin_browser_menu(&mut self, window: &PixelWindow, trigger: ChromeTrigger) {
        self.menu_return_focus = Some(self.current_menu_focus());
        self.blur_browser_composition();
        self.browser_address_focused = false;
        self.browser_proxy_focused = false;
        self.settings_proxy_focused = false;
        self.composer.preedit.clear();
        self.composer.focused = false;
        self.notes_focused = false;
        self.browser_chrome_trigger = Some(trigger);
        self.composer_selecting = false;
        self.cancel_pointer_gestures_for_activation(window);
        let _ = window.set_pointer_capture(false);
        #[cfg(any(target_os = "macos", windows))]
        if let Some(page) = &self.native_browser_page {
            page.focus_parent();
        }
        #[cfg(target_os = "linux")]
        window.focus();
    }

    pub(super) fn close_browser_menu(
        &mut self,
        window: &PixelWindow,
        restore: bool,
        activate: bool,
    ) {
        self.browser_proxy_menu_open = false;
        self.new_tab_menu_open = false;
        let owner = self.menu_return_focus.take();
        self.browser_chrome_trigger = None;
        if restore {
            match owner {
                Some(MenuFocus::Address) => {
                    self.browser_address_focused = self.browser_address.is_some()
                }
                Some(MenuFocus::Proxy) => self.browser_proxy_focused = true,
                Some(MenuFocus::Candidate) => {
                    self.settings_open = true;
                    self.settings_section = ui::SettingsSection::Web;
                    self.settings_proxy_focused = true;
                }
                Some(MenuFocus::Settings) => self.settings_open = true,
                Some(MenuFocus::NewTrigger) => {
                    self.browser_chrome_trigger = Some(ChromeTrigger::New)
                }
                Some(MenuFocus::ProxyTrigger) => {
                    self.browser_chrome_trigger = Some(ChromeTrigger::Proxy)
                }
                Some(MenuFocus::Composer) => self.composer.focused = true,
                Some(MenuFocus::Notes) => self.notes_focused = true,
                _ => {}
            }
        }
        if restore && !activate {
            // Deactivation records the logical target without activating a
            // background window. A later real FocusChanged(true) resumes it.
            self.menu_return_focus = owner;
        }
        self.sync_embedded_page(window);
        if restore && activate {
            if owner == Some(MenuFocus::Page) {
                self.focus_browser_page(window);
            } else {
                #[cfg(any(target_os = "macos", windows))]
                if let Some(page) = &self.native_browser_page {
                    page.focus_parent();
                }
                #[cfg(target_os = "linux")]
                window.focus();
                if self.browser_address_focused {
                    self.update_browser_ime_anchor(window);
                }
                if self.browser_proxy_focused {
                    self.update_proxy_ime_anchor(window);
                }
                if self.settings_proxy_focused {
                    self.update_settings_proxy_anchor(window);
                }
                if self.composer.focused {
                    let _ = self.update_composer_ime_anchor(window);
                }
            }
        }
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn focus_browser_page(&self, window: &PixelWindow) {
        if !self.browser_page_visible() {
            return;
        }
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

    pub(super) fn menu_resize_notice(&mut self, window: &PixelWindow) {
        self.note_host_notice(
            match self.ui_language {
                ui::UiLanguage::English => "Resize the window to show this menu.",
                ui::UiLanguage::ChineseSimplified => "请调整窗口大小以显示菜单。",
                ui::UiLanguage::ChineseTraditional => "請調整視窗大小以顯示選單。",
            }
            .to_owned(),
            window,
        );
    }

    pub(super) fn route_browser_chrome(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if matches!(event, PixelWindowEvent::FocusChanged(true))
            && !self.new_tab_menu_open
            && !self.browser_proxy_menu_open
            && self.menu_return_focus.is_some()
        {
            self.close_browser_menu(window, true, true);
        }
        if !self.active_is_browser() || self.settings_open {
            self.browser_chrome_trigger = None;
            return Ok(false);
        }
        if matches!(
            event,
            PixelWindowEvent::PointerButton {
                state: PointerButtonState::Pressed,
                ..
            }
        ) {
            self.browser_chrome_trigger = None;
        }
        let PixelWindowEvent::Keyboard(key) = event else {
            return Ok(false);
        };
        if key.state != KeyPressState::Pressed
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.meta
        {
            return Ok(false);
        }
        let Some(named) = menu_key(key) else {
            return Ok(false);
        };
        if named == NamedKey::Tab {
            let current = if self.browser_address_focused {
                1
            } else if self.browser_proxy_focused {
                2
            } else if let Some(trigger) = self.browser_chrome_trigger {
                if trigger == ChromeTrigger::New { 0 } else { 3 }
            } else {
                return Ok(false);
            };
            let target = (current + if key.modifiers.shift { 3 } else { 1 }) % 4;
            self.blur_browser_composition();
            match target {
                1 => self.focus_browser_address(window),
                2 => self.focus_browser_proxy(window),
                _ => {
                    self.browser_address_focused = false;
                    self.browser_proxy_focused = false;
                    self.settings_proxy_focused = false;
                    self.browser_chrome_trigger = Some(if target == 0 {
                        ChromeTrigger::New
                    } else {
                        ChromeTrigger::Proxy
                    });
                    self.mark_host_ui_full_and_repaint(window);
                }
            }
            return Ok(true);
        }
        if matches!(named, NamedKey::Enter | NamedKey::Space)
            && let Some(trigger) = self.browser_chrome_trigger
        {
            self.menu_key_latch = Some(named);
            match trigger {
                ChromeTrigger::New => self.toggle_new_tab_menu(window),
                ChromeTrigger::Proxy => {
                    let metrics = window.metrics()?;
                    self.open_proxy_choices(window, metrics);
                }
            }
            return Ok(true);
        }
        Ok(self.browser_chrome_trigger.is_some())
    }

    pub(super) fn consume_menu_gesture(&mut self, event: &PixelWindowEvent) -> bool {
        match event {
            PixelWindowEvent::PointerButton { button, state, .. }
                if self.menu_pointer_latch.as_ref() == Some(button) =>
            {
                if *state == PointerButtonState::Released {
                    self.menu_pointer_latch = None;
                }
                true
            }
            PixelWindowEvent::Keyboard(key)
                if menu_key(key).is_some_and(|named| self.menu_key_latch == Some(named)) =>
            {
                if key.state == KeyPressState::Released {
                    self.menu_key_latch = None;
                }
                true
            }
            PixelWindowEvent::FocusChanged(false) | PixelWindowEvent::PointerCaptureLost => {
                self.menu_pointer_latch = None;
                self.menu_key_latch = None;
                false
            }
            _ => false,
        }
    }

    fn open_proxy_choices(
        &mut self,
        window: &PixelWindow,
        metrics: minicon_platform::window_host::PixelWindowMetrics,
    ) {
        let scale = metrics.scale_factor.max(1.0);
        let choices = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .browser_proxy_menu(scale, self.config.browser_proxy_options.len() + 1, 0);
        if choices.visible_rows == 0 || choices.rect.width < (96.0 * scale) as u32 {
            self.menu_resize_notice(window);
            return;
        }
        self.begin_browser_menu(window, ChromeTrigger::Proxy);
        self.proxy_menu_highlight = self
            .config
            .browser_proxy_options
            .iter()
            .position(|value| value == &self.browser_proxy_applied)
            .map_or(0, |index| index + 1);
        self.proxy_menu_scroll = choices.reveal(self.proxy_menu_highlight);
        self.browser_proxy_menu_open = true;
        self.new_tab_menu_open = false;
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn route_proxy_choices(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if self.browser_proxy_menu_open {
            return self.route_open_browser_menu(window, event, ChromeTrigger::Proxy);
        }
        if self.settings_open || self.new_tab_menu_open || !self.active_is_browser() {
            return Ok(false);
        }
        if let PixelWindowEvent::PointerButton {
            button: PointerButton::Left,
            state: PointerButtonState::Pressed,
            position: Some(position),
            ..
        } = event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
            if layout.browser_proxy_dropdown(scale).contains(
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            ) {
                self.menu_pointer_latch = Some(PointerButton::Left);
                self.open_proxy_choices(window, metrics);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn route_open_browser_menu(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
        kind: ChromeTrigger,
    ) -> Result<bool, PixelWindowError> {
        if kind == ChromeTrigger::Proxy
            && (!self.active_is_browser() || self.settings_open || self.new_tab_menu_open)
        {
            self.close_browser_menu(window, false, false);
            return Ok(false);
        }
        if matches!(event, PixelWindowEvent::FocusChanged(false)) {
            self.close_browser_menu(window, true, false);
            return Ok(false);
        }
        let metrics = match event {
            PixelWindowEvent::GeometryChanged { metrics, .. } => *metrics,
            _ => window.metrics()?,
        };
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
        let (count, highlight, start) = match kind {
            ChromeTrigger::New => (2, self.new_menu_highlight, self.new_menu_scroll),
            ChromeTrigger::Proxy => (
                self.config.browser_proxy_options.len() + 1,
                self.proxy_menu_highlight,
                self.proxy_menu_scroll,
            ),
        };
        let choices = match kind {
            ChromeTrigger::New => layout.new_tab_choices(scale, start),
            ChromeTrigger::Proxy => layout.browser_proxy_menu(scale, count, start),
        };
        if choices.visible_rows == 0 || choices.rect.width < (96.0 * scale) as u32 {
            self.close_browser_menu(window, true, true);
            self.menu_resize_notice(window);
            return Ok(false);
        }
        if matches!(event, PixelWindowEvent::GeometryChanged { .. }) {
            self.set_menu_position(
                kind,
                highlight.min(count - 1),
                choices.reveal(highlight.min(count - 1)),
            );
            self.mark_host_ui_full_and_repaint(window);
            return Ok(false);
        }
        match event {
            PixelWindowEvent::PointerButton {
                button,
                state: PointerButtonState::Pressed,
                position,
                ..
            } => {
                self.menu_pointer_latch = Some(*button);
                let selected = if *button == PointerButton::Left {
                    position.as_ref().and_then(|position| {
                        choices.hit(
                            (position.x * scale).max(0.0) as u32,
                            (position.y * scale).max(0.0) as u32,
                        )
                    })
                } else {
                    None
                };
                if let Some(index) = selected {
                    self.activate_browser_menu(window, kind, index);
                } else {
                    self.close_browser_menu(window, true, true);
                }
                // The same outside gesture cannot reach Back/Reload, another
                // trigger, the restored native page or the terminal behind us.
                Ok(true)
            }
            PixelWindowEvent::PointerMoved { position, .. } => {
                if let Some(index) = choices.hit(
                    (position.x * scale).max(0.0) as u32,
                    (position.y * scale).max(0.0) as u32,
                ) {
                    self.set_menu_position(kind, index, choices.start);
                    self.mark_host_ui_full_and_repaint(window);
                }
                Ok(true)
            }
            PixelWindowEvent::MouseWheel { delta, .. } => {
                let rows = match delta {
                    WheelDelta::Lines { y, .. } => minicon_core::numeric::round_f32(*y) as isize,
                    WheelDelta::LogicalPixels { y, .. } => {
                        minicon_core::numeric::round_f64(*y / 32.0) as isize
                    }
                    _ => 0,
                };
                let next = choices
                    .start
                    .saturating_add_signed(rows.saturating_neg())
                    .min(count.saturating_sub(choices.visible_rows));
                self.set_menu_position(
                    kind,
                    highlight.clamp(next, next + choices.visible_rows - 1),
                    next,
                );
                self.mark_host_ui_full_and_repaint(window);
                Ok(true)
            }
            PixelWindowEvent::Keyboard(key) => {
                if key.state == KeyPressState::Pressed {
                    match menu_key(key) {
                        Some(
                            NamedKey::ArrowUp
                            | NamedKey::ArrowDown
                            | NamedKey::Home
                            | NamedKey::End,
                        ) => {
                            let index = match menu_key(key) {
                                Some(NamedKey::ArrowUp) => highlight.saturating_sub(1),
                                Some(NamedKey::ArrowDown) => (highlight + 1).min(count - 1),
                                Some(NamedKey::Home) => 0,
                                _ => count - 1,
                            };
                            self.set_menu_position(kind, index, choices.reveal(index));
                            self.mark_host_ui_full_and_repaint(window);
                        }
                        Some(named @ (NamedKey::Enter | NamedKey::Space)) => {
                            self.menu_key_latch = Some(named);
                            self.activate_browser_menu(window, kind, highlight);
                        }
                        Some(named @ (NamedKey::Escape | NamedKey::Tab)) => {
                            self.menu_key_latch = Some(named);
                            self.close_browser_menu(window, true, true);
                        }
                        _ => {}
                    }
                }
                Ok(true)
            }
            PixelWindowEvent::Ime(_)
            | PixelWindowEvent::PointerButton { .. }
            | PixelWindowEvent::PointerLeft
            | PixelWindowEvent::PointerCaptureLost => Ok(true),
            _ => Ok(false),
        }
    }

    fn set_menu_position(&mut self, kind: ChromeTrigger, highlight: usize, start: usize) {
        match kind {
            ChromeTrigger::New => {
                self.new_menu_highlight = highlight;
                self.new_menu_scroll = start;
            }
            ChromeTrigger::Proxy => {
                self.proxy_menu_highlight = highlight;
                self.proxy_menu_scroll = start;
            }
        }
    }

    fn activate_browser_menu(&mut self, window: &PixelWindow, kind: ChromeTrigger, index: usize) {
        self.close_browser_menu(window, false, false);
        match kind {
            ChromeTrigger::New if index == 0 => self.open_session_contained(window, false),
            ChromeTrigger::New => {
                self.open_browser_tab_contained(window);
                self.focus_browser_address(window);
            }
            ChromeTrigger::Proxy => {
                let text = index
                    .checked_sub(1)
                    .and_then(|index| self.config.browser_proxy_options.get(index))
                    .cloned()
                    .unwrap_or_default();
                self.browser_proxy_input = composer::ComposerState::default();
                self.browser_proxy_input.text = text;
                self.browser_proxy_input.caret = self.browser_proxy_input.text.len();
                self.apply_browser_proxy(window);
                self.focus_browser_page(window);
            }
        }
        self.sync_embedded_page(window);
    }
}
