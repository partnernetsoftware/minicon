//! Visible tab creation and recovery hit routing. Product owner: PRD 25.
use super::*;

impl ConApp {
    pub(super) fn toggle_new_tab_menu(&mut self, window: &PixelWindow) {
        if self.new_tab_menu_open {
            self.close_browser_menu(window, true, true);
            return;
        }
        let Ok(metrics) = window.metrics() else {
            return;
        };
        let scale = metrics.scale_factor.max(1.0);
        let choices = self
            .layout(metrics.physical_width, metrics.physical_height, scale)
            .new_tab_choices(scale, 0);
        if choices.visible_rows == 0 || choices.rect.width < (96.0 * scale) as u32 {
            self.menu_resize_notice(window);
            return;
        }
        self.begin_browser_menu(window, super::browser_menu::ChromeTrigger::New);
        self.new_tab_menu_open = true;
        self.browser_proxy_menu_open = false;
        self.new_menu_highlight = 0;
        self.new_menu_scroll = 0;
        self.settings_open = false;
        self.commit_rename(window);
        self.sync_embedded_page(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    pub(super) fn handle_new_tab_menu(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        if self.new_tab_menu_open {
            return self.route_open_browser_menu(
                window,
                event,
                super::browser_menu::ChromeTrigger::New,
            );
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
            if layout.new_root.contains(
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            ) {
                self.menu_pointer_latch = Some(PointerButton::Left);
                self.toggle_new_tab_menu(window);
                return Ok(true);
            }
        }
        Ok(false)
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
