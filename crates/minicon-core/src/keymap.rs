//! The composer's key rules, as one table.
//!
//! These rules used to live in three places in the binary: a `match` inside
//! the key handler, a commit-action helper, and a decline helper — with the
//! human-readable list written a fourth time in `--help` and the README. Four
//! copies of one decision drift, and the drift is invisible: nothing fails
//! when the help promises a key the handler does not implement.
//!
//! So the table is the decision, [`action`] is the only thing that reads it
//! for behaviour, and [`help_lines`] is the only thing that reads it for
//! prose. Adding a key means adding a row.
//!
//! Two things are deliberately passed in rather than decided here, because
//! this crate must behave identically on every target:
//!
//! - [`ClipboardModifier`] — macOS means Command where every other platform
//!   means Control. A `cfg` would make the macOS rule untestable anywhere
//!   else; a parameter makes it a value in a unit test.
//! - [`Draft`] — where the caret sits decides whether Up moves or recalls.

use crate::composer::Move;

/// A key, named the way the composer thinks about it rather than the way any
/// one windowing system spells it. The host translates its own event into
/// this before asking the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// A character the key would type. Case is not normalised: the table
    /// compares case-insensitively where that is the rule.
    Character(char),
    Space,
    Enter,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl Chord {
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::default(),
        }
    }
}

/// Which modifier means "clipboard action" on this host.
///
/// macOS uses Command — a Mac keyboard has no Insert key and `Ctrl+C` there is
/// not copy — but still accepts Control so existing habits keep working.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClipboardModifier {
    ControlOnly,
    ControlOrMeta,
}

impl ClipboardModifier {
    /// Alt/AltGr never counts, so composed characters still type.
    fn matches(self, modifiers: &Modifiers) -> bool {
        if modifiers.alt {
            return false;
        }
        match self {
            Self::ControlOnly => modifiers.control,
            Self::ControlOrMeta => modifiers.control || modifiers.meta,
        }
    }
}

/// Where the caret sits in the draft. Up and Down are the only rules that need
/// it, and they need it because one key has to serve two jobs: moving inside a
/// multi-line draft, and recalling what was sent before.
///
/// A single-line draft has the caret on both the first and the last line, so
/// it recalls in both directions — which is the behaviour every shell has and
/// the one a user of this product already has in their fingers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Draft {
    pub caret_on_first_line: bool,
    pub caret_on_last_line: bool,
}

impl Draft {
    /// The shape of a draft with no newline in it, whatever the caret offset.
    pub fn single_line() -> Self {
        Self {
            caret_on_first_line: true,
            caret_on_last_line: true,
        }
    }
}

/// What the host should do with a key.
///
/// [`Action::Decline`] is distinct from `None`: declining hands the key to the
/// terminal handler, while `None` consumes it and does nothing. Scrollback
/// paging declines so that history scrolls identically whether the composer or
/// the terminal has focus; an unbound `Ctrl+Z` is consumed so it does not
/// reach the shell from a text box the user is typing in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Decline,
    SoftNewline,
    Send,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Backspace,
    DeleteForward,
    DeleteWordBack,
    DeleteWordForward,
    Move(Move),
    Extend(Move),
    RecallPrevious,
    RecallNext,
    CancelFocus,
    /// Type the key's own text. The host holds the text; the table only
    /// decides that typing is what should happen.
    Insert,
}

/// The one place a composer key becomes a decision.
///
/// `None` means "consumed, do nothing" — see [`Action::Decline`] for the other
/// kind of not-handling.
pub fn action(chord: Chord, clipboard: ClipboardModifier, draft: Draft) -> Option<Action> {
    let modifiers = chord.modifiers;

    // Scrollback paging belongs to the terminal in both focus states.
    if modifiers.shift && matches!(chord.key, Key::PageUp | Key::PageDown) {
        return Some(Action::Decline);
    }

    // Commit actions outrank everything else, and neither takes Alt or Meta.
    if !modifiers.alt && !modifiers.meta {
        if !modifiers.control && chord.key == Key::Enter {
            return Some(Action::SoftNewline);
        }
        if modifiers.control
            && !modifiers.shift
            && matches!(chord.key, Key::Character(c) if c.eq_ignore_ascii_case(&'o'))
        {
            return Some(Action::Send);
        }
    }

    // The clipboard modifier claims every character key, bound or not, so an
    // unbound one cannot fall through and type itself.
    if clipboard.matches(&modifiers)
        && let Key::Character(character) = chord.key
    {
        return match character.to_ascii_lowercase() {
            'a' => Some(Action::SelectAll),
            'c' => Some(Action::Copy),
            'x' => Some(Action::Cut),
            'v' => Some(Action::Paste),
            _ => None,
        };
    }

    match chord.key {
        // Control turns a character delete into a word delete, matching the
        // span Ctrl+Left/Right would have moved over.
        Key::Backspace if modifiers.control => Some(Action::DeleteWordBack),
        Key::Delete if modifiers.control => Some(Action::DeleteWordForward),
        Key::Backspace => Some(Action::Backspace),
        Key::Delete => Some(Action::DeleteForward),
        Key::Escape => Some(Action::CancelFocus),

        // Up and Down serve two jobs. Alt asks for history explicitly, Shift
        // selects by line, and otherwise the caret moves while there is a line
        // to move to — falling back to recall at the edge of the draft, so a
        // single-line draft behaves exactly as it always did.
        Key::Up => Some(if modifiers.alt {
            Action::RecallPrevious
        } else if modifiers.shift {
            Action::Extend(Move::Up)
        } else if draft.caret_on_first_line {
            Action::RecallPrevious
        } else {
            Action::Move(Move::Up)
        }),
        Key::Down => Some(if modifiers.alt {
            Action::RecallNext
        } else if modifiers.shift {
            Action::Extend(Move::Down)
        } else if draft.caret_on_last_line {
            Action::RecallNext
        } else {
            Action::Move(Move::Down)
        }),

        // Control widens each horizontal motion: by word instead of by
        // character, and to the whole draft instead of to the line. Shift
        // still decides whether the motion moves or selects, so the two
        // modifiers compose rather than override each other.
        Key::Left => Some(movement(
            modifiers,
            if modifiers.control {
                Move::WordLeft
            } else {
                Move::Left
            },
        )),
        Key::Right => Some(movement(
            modifiers,
            if modifiers.control {
                Move::WordRight
            } else {
                Move::Right
            },
        )),
        Key::Home => Some(movement(
            modifiers,
            if modifiers.control {
                Move::DraftStart
            } else {
                Move::LineStart
            },
        )),
        Key::End => Some(movement(
            modifiers,
            if modifiers.control {
                Move::DraftEnd
            } else {
                Move::LineEnd
            },
        )),

        Key::Space if !modifiers.control && !modifiers.alt => Some(Action::Insert),
        Key::Character(_) if !modifiers.control && !modifiers.alt => Some(Action::Insert),
        _ => None,
    }
}

fn movement(modifiers: Modifiers, movement: Move) -> Action {
    if modifiers.shift {
        Action::Extend(movement)
    } else {
        Action::Move(movement)
    }
}

/// One row of the human-readable list, so `--help`, the README and the
/// in-product guide cannot promise a key the table does not implement.
pub struct Binding {
    pub chord: &'static str,
    pub description: &'static str,
}

/// The composer keys worth telling a human about. Movement keys that behave
/// the way every text box behaves are left out on purpose — a help list that
/// documents Left and Right teaches nothing and hides the two lines that do.
pub const HELP: &[Binding] = &[
    Binding {
        chord: "Enter",
        description: "Insert a soft newline in the input area",
    },
    Binding {
        chord: "Ctrl+O",
        description: "Send the complete input-area draft",
    },
    Binding {
        chord: "Up / Down",
        description: "Move between lines, or recall what you sent before",
    },
    Binding {
        chord: "Alt+Up / Down",
        description: "Recall what you sent before, from any line",
    },
    Binding {
        chord: "Ctrl+Left / Right",
        description: "Move the input caret by whole words",
    },
    Binding {
        chord: "Ctrl+Home / End",
        description: "Move the input caret to the start or end of the draft",
    },
    Binding {
        chord: "Ctrl+Backspace / Delete",
        description: "Delete the word before or after the input caret",
    },
];

/// The help list as the lines `--help` prints, so the formatting lives with
/// the table rather than at each call site.
///
/// The column is as wide as the widest chord in the table, with a floor at the
/// width the surrounding help block already uses. Measuring it means a long
/// new chord widens the column instead of overflowing it, which a hard-coded
/// width did on the first one added.
pub fn help_lines() -> impl Iterator<Item = String> {
    const SURROUNDING_COLUMN: usize = 18;
    let width = HELP
        .iter()
        .map(|binding| binding.chord.chars().count())
        .max()
        .unwrap_or(0)
        // One past the widest chord, so even that row keeps a gap rather than
        // running into its own description.
        .saturating_add(1)
        .max(SURROUNDING_COLUMN);
    HELP.iter()
        .map(move |binding| format!("  {:<width$} {}", binding.chord, binding.description))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: ClipboardModifier = ClipboardModifier::ControlOrMeta;
    const PC: ClipboardModifier = ClipboardModifier::ControlOnly;

    fn multiline_middle() -> Draft {
        Draft {
            caret_on_first_line: false,
            caret_on_last_line: false,
        }
    }

    fn chord(key: Key, modifiers: Modifiers) -> Chord {
        Chord { key, modifiers }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::default()
        }
    }

    fn meta() -> Modifiers {
        Modifiers {
            meta: true,
            ..Modifiers::default()
        }
    }

    /// The owner's report against 0.1.22: after pasting several lines, the
    /// arrow keys could not reach the earlier ones because Up recalled history
    /// instead of moving the caret.
    #[test]
    fn up_moves_the_caret_inside_a_multi_line_draft() {
        assert_eq!(
            action(Chord::plain(Key::Up), PC, multiline_middle()),
            Some(Action::Move(Move::Up))
        );
        assert_eq!(
            action(Chord::plain(Key::Down), PC, multiline_middle()),
            Some(Action::Move(Move::Down))
        );
    }

    /// The invariant that makes the change safe to ship: a one-line draft is
    /// what a user types nearly every time, and it must keep recalling.
    #[test]
    fn a_single_line_draft_still_recalls_on_up() {
        assert_eq!(
            action(Chord::plain(Key::Up), PC, Draft::single_line()),
            Some(Action::RecallPrevious)
        );
        assert_eq!(
            action(Chord::plain(Key::Down), PC, Draft::single_line()),
            Some(Action::RecallNext)
        );
    }

    /// At the top of a multi-line draft there is no line above, so the key
    /// falls through to history rather than doing nothing. The same rule read
    /// from the other end gives Down its behaviour on the last line.
    #[test]
    fn the_edges_of_a_multi_line_draft_reach_history() {
        let first = Draft {
            caret_on_first_line: true,
            caret_on_last_line: false,
        };
        let last = Draft {
            caret_on_first_line: false,
            caret_on_last_line: true,
        };
        assert_eq!(
            action(Chord::plain(Key::Up), PC, first),
            Some(Action::RecallPrevious)
        );
        assert_eq!(
            action(Chord::plain(Key::Down), PC, first),
            Some(Action::Move(Move::Down))
        );
        assert_eq!(
            action(Chord::plain(Key::Up), PC, last),
            Some(Action::Move(Move::Up))
        );
        assert_eq!(
            action(Chord::plain(Key::Down), PC, last),
            Some(Action::RecallNext)
        );
    }

    /// Alt is the escape hatch: from the middle of a long draft, history is
    /// otherwise unreachable without walking the caret to an edge.
    #[test]
    fn alt_recalls_from_anywhere_in_the_draft() {
        assert_eq!(
            action(chord(Key::Up, alt()), PC, multiline_middle()),
            Some(Action::RecallPrevious)
        );
        assert_eq!(
            action(chord(Key::Down, alt()), PC, multiline_middle()),
            Some(Action::RecallNext)
        );
    }

    #[test]
    fn shift_selects_by_line_regardless_of_where_the_caret_is() {
        for draft in [Draft::single_line(), multiline_middle()] {
            assert_eq!(
                action(chord(Key::Up, shift()), PC, draft),
                Some(Action::Extend(Move::Up))
            );
            assert_eq!(
                action(chord(Key::Down, shift()), PC, draft),
                Some(Action::Extend(Move::Down))
            );
        }
    }

    #[test]
    fn horizontal_movement_does_not_depend_on_the_draft_shape() {
        for draft in [Draft::single_line(), multiline_middle()] {
            assert_eq!(
                action(Chord::plain(Key::Left), PC, draft),
                Some(Action::Move(Move::Left))
            );
            assert_eq!(
                action(chord(Key::End, shift()), PC, draft),
                Some(Action::Extend(Move::LineEnd))
            );
        }
    }

    #[test]
    fn enter_is_a_newline_and_ctrl_o_sends() {
        assert_eq!(
            action(Chord::plain(Key::Enter), PC, Draft::single_line()),
            Some(Action::SoftNewline)
        );
        assert_eq!(
            action(chord(Key::Character('o'), ctrl()), PC, Draft::single_line()),
            Some(Action::Send)
        );
        assert_eq!(
            action(chord(Key::Character('O'), ctrl()), PC, Draft::single_line()),
            Some(Action::Send)
        );
    }

    /// Command is the clipboard modifier on macOS and nowhere else. Passing it
    /// as a value rather than reading a `cfg` is what lets this run on the
    /// machine the tests happen to be on.
    #[test]
    fn command_is_a_clipboard_modifier_only_where_the_host_says_so() {
        let copy = chord(Key::Character('c'), meta());
        assert_eq!(action(copy, MAC, Draft::single_line()), Some(Action::Copy));
        // Elsewhere Meta+C is not copy, and Meta is not a typing modifier
        // either, so the character must not reach the draft.
        assert_eq!(action(copy, PC, Draft::single_line()), Some(Action::Insert));
    }

    /// An unbound chord under the clipboard modifier is consumed, not typed.
    /// Letting it fall through would put a bare `z` in the draft when the user
    /// reached for undo.
    #[test]
    fn an_unbound_clipboard_chord_types_nothing() {
        assert_eq!(
            action(chord(Key::Character('z'), ctrl()), PC, Draft::single_line()),
            None
        );
    }

    /// Declining is not the same as ignoring: the terminal must still get it.
    #[test]
    fn scrollback_paging_goes_to_the_terminal() {
        assert_eq!(
            action(chord(Key::PageUp, shift()), PC, Draft::single_line()),
            Some(Action::Decline)
        );
        assert_eq!(
            action(chord(Key::PageDown, shift()), PC, Draft::single_line()),
            Some(Action::Decline)
        );
        // Without Shift it is not the paging chord, and the composer has no
        // use for it.
        assert_eq!(
            action(Chord::plain(Key::PageUp), PC, Draft::single_line()),
            None
        );
    }

    #[test]
    fn ordinary_characters_and_space_type_themselves() {
        assert_eq!(
            action(Chord::plain(Key::Character('a')), PC, Draft::single_line()),
            Some(Action::Insert)
        );
        assert_eq!(
            action(Chord::plain(Key::Space), PC, Draft::single_line()),
            Some(Action::Insert)
        );
        assert_eq!(
            action(chord(Key::Space, ctrl()), PC, Draft::single_line()),
            None
        );
    }

    #[test]
    fn control_widens_horizontal_motion_to_whole_words() {
        for draft in [Draft::single_line(), multiline_middle()] {
            assert_eq!(
                action(chord(Key::Left, ctrl()), PC, draft),
                Some(Action::Move(Move::WordLeft))
            );
            assert_eq!(
                action(chord(Key::Right, ctrl()), PC, draft),
                Some(Action::Move(Move::WordRight))
            );
        }
    }

    #[test]
    fn control_widens_home_and_end_to_the_whole_draft() {
        assert_eq!(
            action(chord(Key::Home, ctrl()), PC, multiline_middle()),
            Some(Action::Move(Move::DraftStart))
        );
        assert_eq!(
            action(chord(Key::End, ctrl()), PC, multiline_middle()),
            Some(Action::Move(Move::DraftEnd))
        );
        // Without Control they stay line-relative, which is the whole point of
        // having both.
        assert_eq!(
            action(Chord::plain(Key::Home), PC, multiline_middle()),
            Some(Action::Move(Move::LineStart))
        );
    }

    /// Control and Shift compose: Control picks how far, Shift picks whether
    /// the motion selects. Overriding one with the other would cost the user
    /// the only way to select a word from the keyboard.
    #[test]
    fn control_and_shift_compose_rather_than_override() {
        let both = Modifiers {
            control: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            action(chord(Key::Right, both), PC, multiline_middle()),
            Some(Action::Extend(Move::WordRight))
        );
        assert_eq!(
            action(chord(Key::Home, both), PC, multiline_middle()),
            Some(Action::Extend(Move::DraftStart))
        );
    }

    #[test]
    fn control_turns_a_character_delete_into_a_word_delete() {
        assert_eq!(
            action(chord(Key::Backspace, ctrl()), PC, Draft::single_line()),
            Some(Action::DeleteWordBack)
        );
        assert_eq!(
            action(chord(Key::Delete, ctrl()), PC, Draft::single_line()),
            Some(Action::DeleteWordForward)
        );
        assert_eq!(
            action(Chord::plain(Key::Backspace), PC, Draft::single_line()),
            Some(Action::Backspace)
        );
        assert_eq!(
            action(Chord::plain(Key::Delete), PC, Draft::single_line()),
            Some(Action::DeleteForward)
        );
    }

    /// The point of the table: the list a human reads is generated from it, so
    /// a row cannot promise a key the resolver does not answer.
    #[test]
    fn every_documented_chord_is_one_the_table_answers() {
        for binding in HELP {
            assert!(
                !binding.chord.is_empty() && !binding.description.is_empty(),
                "a help row must say both what the chord is and what it does"
            );
        }
        // Each documented chord, spelled as the resolver sees it.
        let documented = [
            (Chord::plain(Key::Enter), Draft::single_line()),
            (chord(Key::Character('o'), ctrl()), Draft::single_line()),
            (Chord::plain(Key::Up), multiline_middle()),
            (chord(Key::Up, alt()), multiline_middle()),
            (chord(Key::Left, ctrl()), multiline_middle()),
            (chord(Key::Right, ctrl()), multiline_middle()),
            (chord(Key::Home, ctrl()), multiline_middle()),
            (chord(Key::End, ctrl()), multiline_middle()),
            (chord(Key::Backspace, ctrl()), multiline_middle()),
            (chord(Key::Delete, ctrl()), multiline_middle()),
        ];
        for (chord, draft) in documented {
            assert!(
                action(chord, PC, draft).is_some(),
                "{chord:?} is documented but the table answers nothing"
            );
        }
        assert_eq!(help_lines().count(), HELP.len());

        // Every rendered line puts its description in the same column, so the
        // block stays a table when a long chord joins it.
        let columns: Vec<usize> = HELP
            .iter()
            .zip(help_lines())
            .map(|(binding, line)| {
                line.find(binding.description)
                    .expect("the line contains its description")
            })
            .collect();
        assert!(
            columns.windows(2).all(|pair| pair[0] == pair[1]),
            "descriptions do not share a column: {columns:?}"
        );
    }
}
