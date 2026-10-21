//! Pointer gestures over the window chrome: tree hits, the sidebar resize
//! grip, pointer-surface attribution and the pointer-capture cancellation
//! that a tab switch or close must perform.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use super::*;

impl ConApp {
    pub(super) fn cancel_pointer_gesture_for_tab(
        &mut self,
        window: &PixelWindow,
        id: workspace::TabId,
    ) {
        if self.control_pointer_owner == Some(id) {
            self.control_pointer_owner = None;
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.cancel_pointer_gesture(window);
        }
    }

    pub(super) fn cancel_pointer_gestures_for_activation(&mut self, window: &PixelWindow) {
        let owner = self.control_pointer_owner;
        if let Some(owner) = owner {
            self.cancel_pointer_gesture_for_tab(window, owner);
        }
        if let Some(active) = self.workspace.active()
            && Some(active) != owner
        {
            self.cancel_pointer_gesture_for_tab(window, active);
        }
    }

    pub(super) fn handle_tree_pointer(
        &mut self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let physical_x = (position.x * scale).max(0.0) as u32;
        let physical_y = (position.y * scale).max(0.0) as u32;
        let ids: Vec<_> = self.workspace.nodes().iter().map(|node| node.id).collect();
        match ui::tree_hit(
            layout,
            physical_x,
            physical_y,
            self.tree_scroll_offset,
            ids.len(),
            scale,
        ) {
            ui::TreeHit::Outside => return Ok(false),
            ui::TreeHit::Background => return Ok(true),
            ui::TreeHit::NewRoot => {
                self.settings_open = false;
                self.open_session_contained(window, false);
                return Ok(true);
            }
            ui::TreeHit::SidebarToggle => {
                self.sidebar_collapsed = !self.sidebar_collapsed;
                // Collapsing while the settings panel is open would leave the
                // panel floating over a rail that no longer has room for it.
                self.settings_open = false;
                self.hovered_tree_row = None;
                self.apply_sidebar_width(window)?;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            ui::TreeHit::PanelToggle => {
                self.set_history_panel(!self.history_panel_open);
                self.apply_panel_width(window)?;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            ui::TreeHit::Settings => {
                self.settings_open = !self.settings_open;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            ui::TreeHit::Close(index) => {
                // The close button is only shown for the active or hovered row,
                // so a click on its region only closes when it was actually
                // visible; otherwise it selects the row like any other click.
                let close_visible = self.hovered_tree_row == Some(index)
                    || self.workspace.active() == Some(ids[index]);
                if close_visible {
                    self.activate_session(window, ids[index]);
                    self.mark_host_ui_full();
                    // close_active_session owns the post-close tree-scroll
                    // clamp for every close origin.
                    self.close_active_session(window)?;
                    return Ok(true);
                }
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
            ui::TreeHit::Edit(index) => {
                // The edit button shows only for the active or hovered row, so
                // a click on its region only starts an edit when it was
                // actually visible; otherwise it selects the row like any
                // other click.
                let edit_visible = self.hovered_tree_row == Some(index)
                    || self.workspace.active() == Some(ids[index]);
                if edit_visible {
                    self.begin_rename(window, ids[index]);
                    return Ok(true);
                }
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
            ui::TreeHit::Child(index) => {
                // Same visibility rule as edit/close: the button acts only
                // when it was actually shown. Activating first makes the
                // clicked row the parent, so this is Ctrl+Shift+N scoped to
                // the row the pointer named rather than whichever tab
                // happened to be active.
                let child_visible = self.hovered_tree_row == Some(index)
                    || self.workspace.active() == Some(ids[index]);
                if child_visible {
                    self.activate_session(window, ids[index]);
                    self.open_session_contained(window, true);
                    return Ok(true);
                }
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
            ui::TreeHit::Select(index) => {
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
        }
        self.apply_active_geometry(metrics)?;
        self.composer.focused = false;
        self.refresh_title(window)?;
        window.focus();
        window.request_redraw();
        Ok(true)
    }

    /// Records which region the pointer just acted on, so paste is scoped to
    /// the terminal content area rather than the tab sidebar.
    ///
    /// Uses the same sidebar rectangle the tree hit test does, so the two
    /// cannot disagree about where the sidebar ends, and it is keyed on the
    /// press position rather than on which branch consumed the click — a right
    /// press in the sidebar is not consumed by the tree today and must still
    /// read as the sidebar here.
    pub(super) fn note_pointer_surface(
        &mut self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<(), PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let x = (position.x * scale).max(0.0) as u32;
        let y = (position.y * scale).max(0.0) as u32;
        self.input_surface = if layout.sidebar.contains(x, y) {
            InputSurface::Sidebar
        } else {
            InputSurface::Terminal
        };
        Ok(())
    }

    pub(super) fn handle_sidebar_resize(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let physical = |position: &LogicalPoint| {
            (
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            )
        };
        match event {
            PixelWindowEvent::PointerMoved { position, .. } if self.sidebar_resizing => {
                self.sidebar_width_logical =
                    ui::sidebar_width_from_pointer(position.x, metrics.logical_size.width);
                let (sidebar_width, panel_width) = self.terminal_inset_widths();
                if let Ok(session) = self.active_session_mut() {
                    Self::configure_host_ui(
                        session,
                        metrics.scale_factor,
                        sidebar_width,
                        panel_width,
                    );
                    session.queue_resize(
                        metrics.physical_width,
                        metrics.physical_height,
                        metrics.scale_factor,
                    );
                }
                let _ = window.set_pointer_cursor(PixelPointerCursor::ResizeHorizontal);
                window.request_redraw();
                Ok(true)
            }
            PixelWindowEvent::PointerMoved { position, .. } => {
                let (x, y) = physical(position);
                let over_grip = layout.sidebar_resize_grip(scale).contains(x, y);
                let _ = window.set_pointer_cursor(if over_grip {
                    PixelPointerCursor::ResizeHorizontal
                } else {
                    PixelPointerCursor::Arrow
                });
                // Track the hovered tree row so the per-row close button appears
                // only for the hovered (or active) row. Repaint only when the
                // hovered row actually changes, so motion over the terminal or
                // an unchanged row costs no frame.
                let count = self.workspace.nodes().len();
                let hit = ui::tree_hit(layout, x, y, self.tree_scroll_offset, count, scale);
                let hovered = match hit {
                    ui::TreeHit::Select(index)
                    | ui::TreeHit::Close(index)
                    | ui::TreeHit::Edit(index)
                    | ui::TreeHit::Child(index) => Some(index),
                    _ => None,
                };
                let hovered_header = match hit {
                    ui::TreeHit::NewRoot => Some(HeaderIcon::NewRoot),
                    ui::TreeHit::Settings => Some(HeaderIcon::Settings),
                    ui::TreeHit::SidebarToggle => Some(HeaderIcon::SidebarToggle {
                        collapsed: layout.sidebar_collapsed,
                    }),
                    ui::TreeHit::PanelToggle => Some(HeaderIcon::PanelToggle {
                        open: layout.panel_open,
                    }),
                    _ => None,
                };
                if hovered != self.hovered_tree_row || hovered_header != self.hovered_header {
                    self.hovered_tree_row = hovered;
                    self.hovered_header = hovered_header;
                    self.mark_host_ui_full();
                    window.request_redraw();
                }
                // Grid crosshair: track the hovered terminal cell. Repaint the
                // terminal (and the status readout) only when the cell changes.
                let fw = metrics.physical_width;
                let fh = metrics.physical_height;
                let crosshair_changed = self
                    .active_session_mut()
                    .map(|session| session.update_crosshair(position, fw, fh))
                    .unwrap_or(false);
                if crosshair_changed {
                    if let Ok(session) = self.active_session_mut() {
                        session.dirty.mark_full();
                    }
                    self.mark_host_ui_full();
                    window.request_redraw();
                }
                Ok(over_grip)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Pressed,
                position: Some(position),
                ..
            } => {
                let (x, y) = physical(position);
                if !layout.sidebar_resize_grip(scale).contains(x, y) {
                    return Ok(false);
                }
                self.sidebar_resizing = true;
                let _ = window.set_pointer_capture(true);
                let _ = window.set_pointer_cursor(PixelPointerCursor::ResizeHorizontal);
                Ok(true)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                ..
            } if std::mem::take(&mut self.sidebar_resizing) => {
                let _ = window.set_pointer_capture(false);
                Ok(true)
            }
            PixelWindowEvent::PointerCaptureLost if std::mem::take(&mut self.sidebar_resizing) => {
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
