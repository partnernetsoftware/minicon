//! Visible tab creation and recovery hit routing. Product owner: PRD 25.
use super::*;

impl ConApp {
    fn close_new_tab_menu(&mut self, window: &PixelWindow) {
        self.new_tab_menu_open = false;
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn toggle_new_tab_menu(&mut self, window: &PixelWindow) {
        self.new_tab_menu_open = !self.new_tab_menu_open;
        self.settings_open = false;
        self.browser_address_drag = None;
        if self.new_tab_menu_open {
            self.commit_rename(window);
            if self.notes_focused {
                self.blur_notes(window);
            }
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
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn handle_new_tab_menu(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
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
            let x = (position.x * scale).max(0.0) as u32;
            let y = (position.y * scale).max(0.0) as u32;
            if layout.new_root.contains(x, y) {
                self.toggle_new_tab_menu(window);
                return Ok(true);
            }
            if self.new_tab_menu_open {
                let terminal = layout.new_terminal_action(scale).contains(x, y);
                let browser = layout.new_browser_action(scale).contains(x, y);
                self.close_new_tab_menu(window);
                if terminal {
                    self.open_session_contained(window, false);
                } else if browser {
                    self.open_browser_tab_contained(window);
                    self.focus_browser_address(window);
                }
                self.sync_embedded_page(window);
                return Ok(true);
            }
        }
        if !self.new_tab_menu_open {
            return Ok(false);
        }
        match event {
            PixelWindowEvent::Keyboard(key) => {
                if key.state == KeyPressState::Pressed {
                    if matches!(key.logical, LogicalKey::Named(NamedKey::Escape)) {
                        self.close_new_tab_menu(window);
                    } else if key.modifiers.control || key.modifiers.meta {
                        self.close_new_tab_menu(window);
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            PixelWindowEvent::Ime(_) | PixelWindowEvent::PointerMoved { .. } => Ok(true),
            PixelWindowEvent::FocusChanged(false) => {
                self.close_new_tab_menu(window);
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn handle_browser_recovery_pointer(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if self.settings_open || self.new_tab_menu_open || !self.active_is_browser() {
            return Ok(false);
        }
        let failed = self
            .workspace
            .browser_tab()
            .and_then(|id| self.browsers.get(&id))
            .is_some_and(|tab| matches!(tab.panel().load, browser::PageLoad::Unavailable { .. }));
        if !failed {
            return Ok(false);
        }
        let PixelWindowEvent::PointerButton {
            button: PointerButton::Left,
            state: PointerButtonState::Pressed,
            position: Some(position),
            ..
        } = event
        else {
            return Ok(false);
        };
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(metrics.physical_width, metrics.physical_height, scale);
        let x = (position.x * scale).max(0.0) as u32;
        let y = (position.y * scale).max(0.0) as u32;
        if layout.browser_retry(scale).contains(x, y) {
            self.retry_browser_page(window);
            return Ok(true);
        }
        if layout.browser_edit_address(scale).contains(x, y) {
            if self
                .workspace
                .browser_tab()
                .and_then(|id| self.browsers.get(&id))
                .is_some_and(|tab| browser_feedback::is_proxy_failure(&tab.panel().load))
            {
                self.focus_browser_proxy(window);
            } else {
                self.focus_browser_address(window);
            }
            return Ok(true);
        }
        Ok(false)
    }
}
