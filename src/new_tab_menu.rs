//! Visible tab creation and recovery hit routing. Product owner: PRD 25.
use super::*;

impl ConApp {
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
            .active_browser_tab()
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
                .active_browser_tab()
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
