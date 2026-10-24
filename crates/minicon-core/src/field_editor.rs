//! Bounded single-line field editing; product actions stay with the caller.
//! Ownership and consumers: PRD 25.02 / PRD 28.
use crate::{composer, keymap};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub preedit_chars: usize,
}

impl Limits {
    pub const ADDRESS: Self = Self {
        max_bytes: 8192,
        preedit_chars: 512,
    };
    pub const PROXY: Self = Self {
        max_bytes: 2048,
        preedit_chars: 512,
    };
}

pub enum Command<'a> {
    Action {
        action: keymap::Action,
        text: Option<&'a str>,
    },
    Preedit(&'a str),
    ClearPreedit,
    Commit(&'a str),
    Blur,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Intent {
    #[default]
    None,
    Submit,
    Cancel,
}

/// An outcome reports editing effects, never whether an input should escape
/// the focused field. The host owns and consumes its entire input gesture.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    pub changed: bool,
    pub intent: Intent,
    pub clipboard: Option<String>,
    pub rejected: bool,
}

pub fn apply(state: &mut composer::ComposerState, command: Command<'_>, limits: Limits) -> Outcome {
    let before = (
        state.text.len(),
        state.caret,
        state.anchor,
        state.select_all,
    );
    let mut outcome = Outcome::default();
    match command {
        Command::Preedit(text) => {
            let next: String = text.chars().take(limits.preedit_chars.min(512)).collect();
            outcome.changed = state.preedit != next;
            state.preedit = next;
        }
        Command::ClearPreedit | Command::Blur => {
            outcome.changed = !state.preedit.is_empty();
            state.preedit.clear();
        }
        Command::Commit(text) => {
            outcome.changed = !state.preedit.is_empty();
            state.preedit.clear();
            outcome.rejected = !replace(state, text, limits.max_bytes);
        }
        Command::Action { action, text } => {
            if !state.preedit.is_empty() {
                if matches!(action, keymap::Action::CancelFocus) {
                    state.preedit.clear();
                    outcome.changed = true;
                }
                return outcome;
            }
            match action {
                keymap::Action::Send | keymap::Action::SoftNewline => {
                    outcome.intent = Intent::Submit
                }
                keymap::Action::CancelFocus => outcome.intent = Intent::Cancel,
                keymap::Action::SelectAll => composer::select_all(state),
                keymap::Action::Copy => {
                    outcome.clipboard = composer::selection_text(state).map(str::to_owned)
                }
                keymap::Action::Cut => outcome.clipboard = composer::cut(state),
                keymap::Action::Insert | keymap::Action::Paste => {
                    if let Some(text) = text {
                        outcome.rejected = !replace(state, text, limits.max_bytes);
                    }
                }
                keymap::Action::Backspace => composer::backspace(state),
                keymap::Action::DeleteForward => composer::delete_forward(state),
                keymap::Action::DeleteWordBack => composer::delete_word_back(state),
                keymap::Action::DeleteWordForward => composer::delete_word_forward(state),
                keymap::Action::Move(movement) => composer::move_caret(state, movement),
                keymap::Action::Extend(movement) => composer::extend_selection(state, movement),
                _ => {}
            }
        }
    }
    outcome.changed |= before
        != (
            state.text.len(),
            state.caret,
            state.anchor,
            state.select_all,
        );
    outcome
}

fn replace(state: &mut composer::ComposerState, input: &str, max_bytes: usize) -> bool {
    let limit = max_bytes.min(composer::PASTE_LIMIT_BYTES);
    let mut text = String::with_capacity(input.len().min(limit));
    for character in input.chars() {
        let character = match character {
            '\r' | '\n' => ' ',
            value => value,
        };
        if text.len().saturating_add(character.len_utf8()) > limit {
            return false;
        }
        text.push(character);
    }
    let selected = composer::selection_bounds(state).map_or(0, |(start, end)| end - start);
    if state
        .text
        .len()
        .saturating_sub(selected)
        .saturating_add(text.len())
        > limit
    {
        return false;
    }
    composer::insert(state, &text);
    // A collapsed pointer anchor is not a selection to replace. Insertion
    // finishes that gesture rather than selecting the newly inserted text.
    state.anchor = None;
    state.select_all = false;
    true
}

fn offset_at_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    offset
}

/// Apply the address field's established single/Shift/double/triple click
/// rules. The returned anchor starts a drag; token/all selection ends it.
pub fn press(
    state: &mut composer::ComposerState,
    offset: usize,
    shift: bool,
    clicks: u8,
) -> Option<usize> {
    let offset = offset_at_boundary(&state.text, offset);
    let anchor = if shift {
        state
            .anchor
            .unwrap_or(if state.select_all { 0 } else { state.caret })
    } else {
        offset
    };
    drag(state, anchor, offset);
    if shift {
        return Some(offset_at_boundary(&state.text, anchor));
    }
    match clicks {
        0 | 1 => Some(offset),
        2 => {
            let (start, end) = composer::word_bounds(&state.text, offset);
            state.anchor = Some(start);
            state.caret = end;
            None
        }
        _ => {
            composer::select_all(state);
            None
        }
    }
}

pub fn drag(state: &mut composer::ComposerState, anchor: usize, offset: usize) {
    state.select_all = false;
    state.anchor = Some(offset_at_boundary(&state.text, anchor));
    state.caret = offset_at_boundary(&state.text, offset);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft(text: &str) -> composer::ComposerState {
        let mut state = composer::ComposerState::default();
        composer::insert(&mut state, text);
        state
    }

    #[test]
    fn bounded_selected_replacement_accepts_shorter_keyboard_paste_and_ime_text() {
        for limits in [Limits::ADDRESS, Limits::PROXY] {
            for mode in 0..3 {
                let mut state = draft(&"a".repeat(limits.max_bytes));
                composer::select_all(&mut state);
                let command = match mode {
                    0 => Command::Action {
                        action: keymap::Action::Insert,
                        text: Some("短"),
                    },
                    1 => Command::Action {
                        action: keymap::Action::Paste,
                        text: Some("短"),
                    },
                    _ => Command::Commit("短"),
                };
                let outcome = apply(&mut state, command, limits);
                assert!(
                    !outcome.rejected,
                    "selected replacement at {} bytes was rejected",
                    limits.max_bytes
                );
                assert_eq!(state.text, "短");
                assert_eq!(state.caret, 3);
                assert_eq!(composer::selection_bounds(&state), None);
            }
        }
    }

    #[test]
    fn oversized_selected_replacement_keeps_the_text_caret_and_selection() {
        let mut state = draft("a中z");
        state.anchor = Some(1);
        state.caret = 4;
        let original = (
            state.text.clone(),
            state.caret,
            state.anchor,
            state.select_all,
        );
        let outcome = apply(
            &mut state,
            Command::Commit("超出"),
            Limits {
                max_bytes: 5,
                preedit_chars: 512,
            },
        );
        assert!(outcome.rejected);
        assert_eq!(
            (state.text, state.caret, state.anchor, state.select_all),
            original
        );
    }

    #[test]
    fn preedit_confirmation_does_not_submit_and_blur_retains_the_draft() {
        let mut state = draft("retained");
        apply(&mut state, Command::Preedit("ni"), Limits::ADDRESS);
        let outcome = apply(
            &mut state,
            Command::Action {
                action: keymap::Action::Send,
                text: None,
            },
            Limits::ADDRESS,
        );
        assert_eq!(outcome.intent, Intent::None);
        assert_eq!(state.text, "retained");
        apply(&mut state, Command::Blur, Limits::ADDRESS);
        assert_eq!(state.preedit, "");
        assert_eq!(state.text, "retained");
        assert_eq!(
            apply(
                &mut state,
                Command::Action {
                    action: keymap::Action::Send,
                    text: None
                },
                Limits::ADDRESS
            )
            .intent,
            Intent::Submit
        );
    }

    #[test]
    fn pointer_token_shift_and_utf8_drag_share_the_address_rules() {
        let mut state = draft("one 中 two");
        assert_eq!(press(&mut state, 5, false, 2), None);
        assert_eq!(composer::selection_text(&state), Some("中"));
        press(&mut state, 0, false, 1);
        assert_eq!(press(&mut state, 7, true, 1), Some(0));
        assert_eq!(composer::selection_text(&state), Some("one 中"));
        drag(&mut state, 5, 6);
        assert_eq!(state.anchor, Some(4));
        assert_eq!(state.caret, 4);
    }

    #[test]
    fn typing_after_a_single_click_does_not_reselect_the_inserted_text() {
        let mut state = draft("xy");
        press(&mut state, 1, false, 1);
        apply(&mut state, Command::Commit("a"), Limits::ADDRESS);
        assert_eq!(composer::selection_bounds(&state), None);
        apply(&mut state, Command::Commit("b"), Limits::ADDRESS);
        assert_eq!(state.text, "xaby");
    }
}
