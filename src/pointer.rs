//! Pointer gestures over the window chrome: tree hits, the sidebar resize
//! grip, pointer-surface attribution and the pointer-capture cancellation
//! that a tab switch or close must perform.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct TreeDrag {
    pub source: workspace::TabId,
    pub revision: u64,
    start_x: f64,
    start_y: f64,
    pub dragging: bool,
    pub proposal: Option<workspace::MovePlacement>,
    pub refusal: Option<workspace::MoveError>,
}

impl TreeDrag {
    fn armed(source: workspace::TabId, revision: u64, point: &LogicalPoint) -> Self {
        Self {
            source,
            revision,
            start_x: point.x,
            start_y: point.y,
            dragging: false,
            proposal: None,
            refusal: None,
        }
    }
}

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
        if self.tree_drag.take().is_some() {
            let _ = window.set_pointer_capture(false);
        }
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
                self.toggle_new_tab_menu(window);
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
            ui::TreeHit::Harness => {
                self.toggle_harness_workspace(window)?;
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
                if !layout.sidebar_collapsed {
                    self.tree_drag = Some(TreeDrag::armed(
                        ids[index],
                        self.workspace.topology_revision(),
                        position,
                    ));
                    return Ok(true);
                }
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

    pub(super) fn route_tree_drag(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        let Some(mut drag) = self.tree_drag else {
            return Ok(false);
        };
        match event {
            PixelWindowEvent::Keyboard(key)
                if key.state == KeyPressState::Pressed
                    && key.logical == LogicalKey::Named(NamedKey::Escape) =>
            {
                self.tree_drag = None;
                if drag.dragging {
                    let _ = window.set_pointer_capture(false);
                }
                self.mark_tree_dirty();
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::FocusChanged(false) => {
                self.tree_drag = None;
                self.mark_tree_dirty();
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::Keyboard(_) | PixelWindowEvent::Ime(_) if drag.dragging => Ok(true),
            PixelWindowEvent::PointerButton {
                state: PointerButtonState::Pressed,
                ..
            } => {
                self.tree_drag = None;
                if drag.dragging {
                    let _ = window.set_pointer_capture(false);
                }
                self.mark_tree_dirty();
                self.request_dirty_redraw(window);
                Ok(false)
            }
            PixelWindowEvent::MouseWheel { .. } if drag.dragging => Ok(true),
            PixelWindowEvent::PointerMoved { position, .. } => {
                if !drag.dragging {
                    let distance_sq =
                        (position.x - drag.start_x).powi(2) + (position.y - drag.start_y).powi(2);
                    if distance_sq < 36.0 {
                        return Ok(true);
                    }
                    if window.set_pointer_capture(true).is_err() {
                        self.tree_drag = None;
                        return Ok(true);
                    }
                    drag.dragging = true;
                }
                let metrics = window.metrics()?;
                let scale = metrics.scale_factor.max(1.0);
                let layout = self.layout(
                    metrics.physical_width,
                    metrics.physical_height,
                    metrics.scale_factor,
                );
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                let top = layout.tree_header_height;
                let bottom = top.saturating_add(
                    layout
                        .tree_row_height
                        .saturating_mul(layout.tree_capacity() as u32),
                );
                let edge = (12.0 * scale) as u32;
                if layout.sidebar.contains(x, y) && y >= top && y < bottom {
                    let scroll = if y < top.saturating_add(edge) {
                        -1
                    } else if y >= bottom.saturating_sub(edge) {
                        1
                    } else {
                        0
                    };
                    if scroll != 0 {
                        self.tree_scroll_offset = ui::scroll_tree(
                            self.tree_scroll_offset,
                            scroll,
                            self.workspace.nodes().len(),
                            layout.tree_capacity(),
                        );
                    }
                }
                drag.proposal = self.tree_drop_at(layout, x, y);
                drag.refusal = drag.proposal.and_then(|proposal| {
                    self.workspace
                        .preview_move(drag.source, proposal, drag.revision)
                        .err()
                });
                if self.workspace.topology_revision() != drag.revision {
                    drag.proposal = None;
                    drag.refusal = Some(workspace::MoveError::Stale);
                }
                self.tree_drag = Some(drag);
                self.mark_tree_dirty();
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                position,
                ..
            } => {
                self.tree_drag = None;
                if drag.dragging {
                    let _ = window.set_pointer_capture(false);
                    if let Some(position) = position {
                        let metrics = window.metrics()?;
                        let scale = metrics.scale_factor.max(1.0);
                        let layout = self.layout(
                            metrics.physical_width,
                            metrics.physical_height,
                            metrics.scale_factor,
                        );
                        let x = (position.x * scale).max(0.0) as u32;
                        let y = (position.y * scale).max(0.0) as u32;
                        if let Some(proposal) = self.tree_drop_at(layout, x, y) {
                            match self
                                .workspace
                                .move_subtree(drag.source, proposal, drag.revision)
                            {
                                Ok(true) => {
                                    self.tree_scroll_offset = ui::clamp_tree_scroll(
                                        self.tree_scroll_offset,
                                        self.workspace.nodes().len(),
                                        layout.tree_capacity(),
                                    );
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    self.host_notice =
                                        Some(format!("Tab move was not applied: {error:?}"))
                                }
                            }
                        }
                    }
                    self.mark_host_ui_full_and_repaint(window);
                } else if let Some(position) = position {
                    let metrics = window.metrics()?;
                    let scale = metrics.scale_factor.max(1.0);
                    let layout = self.layout(
                        metrics.physical_width,
                        metrics.physical_height,
                        metrics.scale_factor,
                    );
                    let x = (position.x * scale).max(0.0) as u32;
                    let y = (position.y * scale).max(0.0) as u32;
                    let index = self
                        .workspace
                        .nodes()
                        .iter()
                        .position(|node| node.id == drag.source);
                    if self.workspace.topology_revision() == drag.revision
                        && matches!(
                            ui::tree_hit(
                                layout,
                                x,
                                y,
                                self.tree_scroll_offset,
                                self.workspace.nodes().len(),
                                scale,
                            ),
                            ui::TreeHit::Select(hit) if Some(hit) == index
                        )
                    {
                        self.activate_session(window, drag.source);
                        self.reveal_active_tree_row(window)?;
                        self.mark_host_ui_full();
                        self.apply_active_geometry(metrics)?;
                        self.composer.focused = false;
                        self.refresh_title(window)?;
                        window.focus();
                        window.request_redraw();
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn tree_drop_at(&self, layout: ui::Layout, x: u32, y: u32) -> Option<workspace::MovePlacement> {
        let top = layout.tree_header_height;
        let capacity = layout.tree_capacity();
        let bottom = top.saturating_add(layout.tree_row_height.saturating_mul(capacity as u32));
        if layout.sidebar_collapsed || !layout.sidebar.contains(x, y) || y < top || y >= bottom {
            return None;
        }
        let row = ((y - top) / layout.tree_row_height) as usize;
        let index = self.tree_scroll_offset.saturating_add(row);
        let Some(target) = self.workspace.nodes().get(index) else {
            return Some(workspace::MovePlacement::RootTail);
        };
        let within = (y - top) % layout.tree_row_height;
        if within < layout.tree_row_height / 4 {
            Some(workspace::MovePlacement::Before(target.id))
        } else if within >= layout.tree_row_height.saturating_mul(3) / 4 {
            Some(workspace::MovePlacement::After(target.id))
        } else {
            Some(workspace::MovePlacement::Into(target.id))
        }
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
        if layout.harness.contains(x, y) {
            return Ok(());
        }
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
                    ui::TreeHit::Harness => Some(HeaderIcon::Harness),
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
