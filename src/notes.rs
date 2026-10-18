//! The notes scratchpad: a free-text, Global-only section of the right-side
//! panel (GitHub #5, the feature both 0.2.11 and 0.2.12 deferred here).
//!
//! Owns the notes editing state: a multiline composer-backed buffer that
//! never reaches the PTY (PTY isolation is structural — the only exits from
//! this buffer are the clipboard and `minicon.json`), focus/IME arbitration
//! against the composer and the rename editor, and the IME anchor that
//! follows the notes caret. While the notes field is focused it — not the
//! composer, not the terminal — owns keyboard and IME input; the routing
//! funnel here is the only place a notes consume becomes a damage-carrying
//! claim (the same boundary [`crate::rename`] established).
//!
//! Persistence is the existing config seam: the text rides `minicon.json`
//! through the same atomic write the preferences use, saved on blur and on
//! every committing gesture, never on a timer.

use super::*;

impl ConApp {
    /// Focuses the notes field, taking keyboard/IME ownership from the
    /// composer (a focused composer is unfocused, exactly as `begin_rename`
    /// does; a live rename is committed first because a click that lands in
    /// notes must not leave an invisible rename dangling).
    /// Scrolls the notes view by `delta` rows, clamped so the tail row is
    /// the farthest the view can go (scrolling past the note's head or
    /// below its tail is not a place a reader can be).
    pub(super) fn scroll_notes(&mut self, delta: isize, visible_rows: usize) {
        let total = self.notes.text.lines().count();
        let max_back = total.saturating_sub(visible_rows.max(1));
        self.notes_scroll =
            (self.notes_scroll as isize + delta).clamp(0, max_back as isize) as usize;
    }

    pub(super) fn focus_notes(&mut self, window: &PixelWindow) {
        self.commit_rename(window);
        self.composer.focused = false;
        self.notes_focused = true;
        let _ = self.update_notes_ime_anchor(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Drops notes focus (Escape or a click elsewhere), persisting the text
    /// on the way out. Escape is "done for now", not "discard": notes are a
    /// scratchpad the user expects to find again, so the blur is a save.
    pub(super) fn blur_notes(&mut self, window: &PixelWindow) {
        if !self.notes_focused {
            return;
        }
        self.notes_focused = false;
        self.persist_notes(window);
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Persists the notes text through the config seam (atomic write to
    /// `minicon.json`, same primitive as the preferences). A failed write is
    /// the same notice the settings panel shows, not a crash: the running
    /// text stays and the next blur tries again.
    pub(super) fn persist_notes(&mut self, window: &PixelWindow) {
        let changed = self.config.notes.as_deref() != Some(self.notes.text.as_str());
        if !changed {
            return;
        }
        self.config.notes = Some(self.notes.text.clone());
        let Some(path) = config_path() else {
            self.note_host_notice(
                "could not save notes: no configuration directory".to_owned(),
                window,
            );
            return;
        };
        if let Err(error) = write_config_to(&path, &self.config) {
            self.note_host_notice(format!("could not save notes: {error}"), window);
        }
    }

    /// Routes a key press to the notes editor.
    ///
    /// Enter is a soft newline (a scratchpad is multiline), Escape defocuses,
    /// everything else edits the buffer. Returns whether the visible state
    /// may have moved; damage is owned by the funnel, not here.
    fn handle_notes_key(&mut self, window: &PixelWindow, key: &NormalizedKeyEvent) -> bool {
        if key.state != KeyPressState::Pressed {
            return false;
        }
        let Some(chord) = composer_chord(key) else {
            return false;
        };
        // Multiline draft shape: the caret's line decides whether Up/Down
        // moves within the notes or declines, exactly like the composer.
        let draft = composer_draft_shape(&self.notes);
        let Some(action) = keymap::action(chord, CLIPBOARD_MODIFIER, draft) else {
            return false;
        };
        match action {
            // Scrollback paging declines to the window level, where the
            // notes have no scrollback of their own to page.
            keymap::Action::Decline => return false,
            keymap::Action::CancelFocus => {
                self.blur_notes(window);
                return true;
            }
            keymap::Action::Send => {
                // Ctrl+O in the composer means "send"; in notes there is
                // nothing to send, so it defocuses-and-saves — the same
                // exit the blur takes.
                self.blur_notes(window);
                return true;
            }
            _ => {}
        }
        let editor = &mut self.notes;
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
                    // Paste keeps newlines: unlike a tab label, a scratchpad
                    // has room for structure.
                    composer::insert(editor, &text);
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
            // Recall is swallowed: a scratchpad has no send history, and the
            // key reaching the terminal from a focused text field is the
            // leak class this whole funnel exists to prevent.
            keymap::Action::RecallPrevious
            | keymap::Action::RecallNext
            | keymap::Action::SoftNewline
            | keymap::Action::Send
            | keymap::Action::CancelFocus
            | keymap::Action::Decline => {}
        }
        // Typing recentres the view on the tail: writing is what the view
        // exists for, and a view left scrolled up while the caret edits the
        // tail is the same stale-window shape the composer windows away.
        self.notes_scroll = 0;
        let _ = self.update_notes_ime_anchor(window);
        true
    }

    fn handle_notes_ime(
        &mut self,
        window: &PixelWindow,
        event: minicon_platform::ime::ImeEvent,
    ) -> bool {
        use minicon_platform::ime::{ImeAction, classify_event};
        match classify_event(event, true) {
            ImeAction::UpdatePreedit { text, .. } => self.notes.preedit = text,
            ImeAction::ClearPreedit => self.notes.preedit.clear(),
            ImeAction::CommitText(text) => {
                self.notes.preedit.clear();
                composer::insert(&mut self.notes, &text);
            }
            ImeAction::None => return false,
            _ => self.notes.preedit.clear(),
        }
        let _ = self.update_notes_ime_anchor(window);
        true
    }

    /// The notes editor's ownership of one keyboard or IME event — the same
    /// single-funnel contract as [`ConApp::route_rename_input`]: the pairing
    /// of "handled the event" and "marked the damage" cannot drift apart
    /// here. Panel damage is the host-UI full mark because the notes section
    /// lives inside the right-side panel.
    pub(super) fn route_notes_input(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Option<TextInputOutcome> {
        if !self.notes_focused {
            return None;
        }
        match event {
            PixelWindowEvent::Keyboard(key) => {
                if self.handle_notes_key(window, key) {
                    self.mark_host_ui_full();
                    self.request_dirty_redraw(window);
                    Some(TextInputOutcome::Changed)
                } else {
                    Some(TextInputOutcome::Unchanged(
                        "notes editor: key release or unmapped chord",
                    ))
                }
            }
            PixelWindowEvent::Ime(ime) => {
                if self.handle_notes_ime(window, ime.clone()) {
                    self.mark_host_ui_full();
                    self.request_dirty_redraw(window);
                    Some(TextInputOutcome::Changed)
                } else {
                    Some(TextInputOutcome::Unchanged("notes editor: IME no-op"))
                }
            }
            _ => None,
        }
    }

    /// Anchors the IME candidate window at the notes caret.
    ///
    /// The notes area's geometry comes from the layout's panel reservation;
    /// the caret row is derived from the buffer, bounded by the visible
    /// rows the area can show.
    fn update_notes_ime_anchor(&self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        if !self.notes_focused {
            return Ok(());
        }
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self
            .layout(
                metrics.physical_width,
                metrics.physical_height,
                metrics.scale_factor,
            )
            .history_panel(scale);
        let font_size = scaled_host_ui_font(
            HOST_UI_TAB_SIZE_PX,
            self.active_session()
                .map_or(12.0, |session| session.font_size_logical),
            scale,
        );
        let cell = font::cell_metrics(font_size);
        let caret = self.notes.caret.min(self.notes.text.len());
        let line = composer::line_index_at(&self.notes.text, caret);
        // The same clamped view offset the painter uses, so the candidate
        // window parks beside the caret row the user actually sees; the
        // final clamp keeps a long note's IME inside the panel.
        let total = self.notes.text.lines().count();
        let max_back = total.saturating_sub(layout.notes_visible_rows);
        let skip = self.notes_scroll.min(max_back);
        let row = line
            .saturating_sub(skip)
            .min(layout.notes_visible_rows.saturating_sub(1));
        let x = f64::from(layout.notes.text_x.saturating_add(cell.width / 2)) / scale;
        let y = f64::from(
            layout.notes.text_y.saturating_add(
                u32::try_from(row)
                    .unwrap_or(u32::MAX)
                    .saturating_mul(cell.height),
            ),
        ) / scale;
        let _ = window.set_ime_cursor_area(minicon_platform::window_host::LogicalRect::new(
            x, y, 2.0, 20.0,
        ));
        Ok(())
    }
}
