//! The inline tab-rename editor.
//!
//! Owns the rename state machine: seeding from the tree label, the
//! single-line key/IME editing surface, commit/cancel, and the IME anchor
//! that follows the editor's caret. While a rename is live, this editor —
//! not the composer, not the terminal — owns keyboard and IME input; the
//! routing funnel here is the only place a rename consume becomes a
//! damage-carrying claim.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use super::*;

/// An in-progress inline tab rename.
///
/// The editor reuses the composer's text state (caret, selection, IME preedit)
/// as a single-line buffer: the key handler never lets a newline into it. It is
/// a scratch copy — the tree changes only on commit, so Escape can discard it.
pub(super) struct RenameState {
    pub(super) target: workspace::TabId,
    pub(super) editor: composer::ComposerState,
}

impl ConApp {
    /// Starts an inline rename of a tab-tree label.
    ///
    /// Seeds the editor with the label the row currently shows, so the user
    /// edits what they can see rather than a hidden default. Keyboard input is
    /// diverted to the editor (see the routing in `event`) until the rename
    /// commits or is cancelled.
    pub(super) fn begin_rename(&mut self, window: &PixelWindow, id: workspace::TabId) {
        let Some(node) = self.workspace.node(id) else {
            return;
        };
        let session = self.session_for(id);
        // Seed with the name line — what the row shows as the tab name — not
        // the terminal title, which lives on its own line and is not editable
        // here.
        let label = host_paint::tree_row_name(
            node.custom_title.as_deref(),
            session.map(|terminal| terminal.program_label()),
            node.title.as_str(),
        );
        let mut editor = composer::ComposerState::default();
        editor.text = label.to_owned();
        editor.caret = editor.text.len();
        // Start fully selected so the first keystroke replaces the old name —
        // the common case is "type the new name", not "append to the old one".
        composer::select_all(&mut editor);
        self.rename = Some(RenameState { target: id, editor });
        self.composer.focused = false;
        let _ = self.update_rename_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Commits the in-progress rename into the workspace, then clears it.
    ///
    /// A no-op when nothing is being renamed; a target tab that closed mid-edit
    /// makes the workspace rename refuse, which is likewise ignored.
    pub(super) fn commit_rename(&mut self, window: &PixelWindow) {
        if let Some(rename) = self.rename.take() {
            self.workspace.rename(rename.target, &rename.editor.text);
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    /// Discards the in-progress rename, leaving the tree unchanged.
    fn cancel_rename(&mut self, window: &PixelWindow) {
        if self.rename.take().is_some() {
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    /// Routes a key press to the inline rename editor.
    ///
    /// Enter (soft newline) and Ctrl+O (send) commit, Escape cancels; everything
    /// else edits the buffer. Enter cannot insert a newline because the buffer
    /// is single-line — the key is the commit affordance here, not a line break.
    ///
    /// Returns whether the editor's visible state may have moved. Arms that can
    /// no-op (Copy with no selection, an unmapped Insert) still claim a change:
    /// the safe direction of this claim is over-marking, which costs one
    /// bounded re-raster, while under-marking is the invisible-frame defect
    /// class. Only exits that provably touch nothing return `false`.
    fn handle_rename_key(&mut self, window: &PixelWindow, key: &NormalizedKeyEvent) -> bool {
        if key.state != KeyPressState::Pressed {
            return false;
        }
        let Some(chord) = composer_chord(key) else {
            return false;
        };
        let Some(action) = keymap::action(chord, CLIPBOARD_MODIFIER, keymap::Draft::single_line())
        else {
            return false;
        };
        match action {
            // Scrollback paging is not a thing while renaming; swallow it.
            keymap::Action::Decline => return false,
            keymap::Action::SoftNewline | keymap::Action::Send => {
                self.commit_rename(window);
                return true;
            }
            keymap::Action::CancelFocus => {
                self.cancel_rename(window);
                return true;
            }
            _ => {}
        }
        let Some(editor) = self.rename.as_mut().map(|rename| &mut rename.editor) else {
            return false;
        };
        match action {
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
                    composer::insert(editor, &single_line_label(&text));
                }
            }
            keymap::Action::Backspace => composer::backspace(editor),
            keymap::Action::DeleteForward => composer::delete_forward(editor),
            keymap::Action::DeleteWordBack => composer::delete_word_back(editor),
            keymap::Action::DeleteWordForward => composer::delete_word_forward(editor),
            keymap::Action::Move(movement) => composer::move_caret(editor, movement),
            keymap::Action::Extend(movement) => composer::extend_selection(editor, movement),
            keymap::Action::Insert => match &key.logical {
                LogicalKey::Named(NamedKey::Space) => composer::insert(editor, " "),
                LogicalKey::Character(text) => composer::insert(editor, text),
                _ => {}
            },
            // Commit / cancel / decline returned above; recall has no history to
            // step through while renaming.
            keymap::Action::SoftNewline
            | keymap::Action::Send
            | keymap::Action::CancelFocus
            | keymap::Action::Decline
            | keymap::Action::RecallPrevious
            | keymap::Action::RecallNext => {}
        }
        // Damage and redraw are owned by the routing funnel, not here: a bare
        // request_redraw here paints the last rasterised frame again (the
        // 0.2.10 damage model skips re-rasterising undamaged host UI), which
        // is exactly how an in-progress rename showed no text or caret. The
        // funnel pairs this `true` with the tree-damage mark.
        let _ = self.update_rename_ime_anchor(window);
        true
    }

    fn handle_rename_ime(
        &mut self,
        window: &PixelWindow,
        event: minicon_platform::ime::ImeEvent,
    ) -> bool {
        use minicon_platform::ime::{ImeAction, classify_event};
        let Some(rename) = self.rename.as_mut() else {
            return false;
        };
        match classify_event(event, true) {
            ImeAction::UpdatePreedit { text, .. } => rename.editor.preedit = text,
            ImeAction::ClearPreedit => rename.editor.preedit.clear(),
            ImeAction::CommitText(text) => {
                rename.editor.preedit.clear();
                composer::insert(&mut rename.editor, &single_line_label(&text));
            }
            ImeAction::None => return false,
            _ => rename.editor.preedit.clear(),
        }
        let _ = self.update_rename_ime_anchor(window);
        true
    }

    /// The rename editor's ownership of one keyboard or IME event.
    ///
    /// While a rename is live it owns the keyboard and IME entirely, so a
    /// keystroke edits the label instead of reaching the terminal; a left
    /// press already committed the rename before this runs, and non-text
    /// events fall through so window-level handling still works.
    ///
    /// Returns `None` when the event was not consumed, and a
    /// [`TextInputOutcome`] — with the tree damage already marked — when it
    /// was. This funnel is the only place a rename consume becomes a claim:
    /// the pairing of "handled the event" and "marked the damage" cannot
    /// drift apart here the way it did at the old inline dispatch site.
    pub(super) fn route_rename_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Option<TextInputOutcome> {
        self.rename.as_ref()?;
        match event {
            PixelWindowEvent::Keyboard(key) => {
                if self.handle_rename_key(window, key) {
                    self.mark_tree_dirty();
                    self.request_dirty_redraw(window);
                    Some(TextInputOutcome::Changed)
                } else {
                    Some(TextInputOutcome::Unchanged(
                        "rename editor: key release or unmapped chord",
                    ))
                }
            }
            PixelWindowEvent::Ime(ime) => {
                if self.handle_rename_ime(window, ime.clone()) {
                    self.mark_tree_dirty();
                    self.request_dirty_redraw(window);
                    Some(TextInputOutcome::Changed)
                } else {
                    Some(TextInputOutcome::Unchanged("rename editor: IME no-op"))
                }
            }
            _ => None,
        }
    }

    /// Anchors the IME candidate window at the inline rename editor's caret.
    ///
    /// Mirrors [`ConApp::update_composer_ime_anchor`], but on a tab row instead
    /// of the composer: the row's indent sets the text origin and the caret's
    /// visible cell sets the horizontal offset.
    fn update_rename_ime_anchor(&self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let Some(rename) = self.rename.as_ref() else {
            return Ok(());
        };
        let Some(node_index) = self
            .workspace
            .nodes()
            .iter()
            .position(|node| node.id == rename.target)
        else {
            return Ok(());
        };
        let Some(visible_row) = node_index.checked_sub(self.tree_scroll_offset) else {
            return Ok(());
        };
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let font_size = scaled_host_ui_font(
            HOST_UI_TAB_SIZE_PX,
            self.active_session()?.font_size_logical,
            scale,
        );
        let cell_w = font::cell_metrics(font_size).width.max(1);
        let depth = self
            .workspace
            .depths()
            .get(node_index)
            .copied()
            .unwrap_or(0)
            .min(8);
        let indent = 14 + depth * 18;
        let avail = layout
            .sidebar
            .width
            .saturating_sub(8)
            .saturating_sub(indent);
        let cells = (avail / cell_w).max(1) as usize;
        let window_text = composer::visible_window(
            &rename.editor.text,
            &rename.editor.preedit,
            rename.editor.caret,
            1,
            cells,
        );
        let caret = rename
            .editor
            .caret
            .clamp(window_text.text, rename.editor.text.len());
        let caret_cells = usize::from(window_text.truncated)
            + composer::cells(&rename.editor.text[window_text.text..caret])
            + composer::cells(&rename.editor.preedit[window_text.preedit..]);
        let row_y = layout.tree_header_height.saturating_add(
            u32::try_from(visible_row)
                .unwrap_or(u32::MAX)
                .saturating_mul(layout.tree_row_height),
        );
        let x = f64::from(indent.saturating_add(cell_w.saturating_mul(caret_cells as u32))) / scale;
        let y = f64::from(row_y.saturating_add(7)) / scale;
        let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
            x, y, 2.0, 20.0,
        ));
        Ok(())
    }
}
