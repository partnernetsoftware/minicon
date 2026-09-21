//! Latin-or-native for macOS input methods, inferred from what they do.
//!
//! Windows answers "is the IME in its native script?" directly
//! (`ImmGetConversionStatus`). macOS does not: the platform adapter can only
//! see which input *source* is selected, and switching an input method between
//! Chinese and English with Shift -- 微信输入法, 搜狗, Apple Pinyin -- happens
//! inside the input method's own process without changing the source. So the
//! status bar read "native" whatever the user had toggled.
//!
//! What MiniCon can see is the input method's behaviour on the keys the user
//! types:
//!
//! - a letter that starts a composition (a non-empty preedit) means native;
//! - a letter that arrives as plain text, with no composition, means latin.
//!
//! So after a Shift toggle the readout follows on the next letter typed. Acting
//! on the Shift itself was considered and left out: a lone Shift reaches MiniCon
//! through winit as an unidentified modifier key indistinguishable from a lone
//! Cmd or Ctrl, so "flip on Shift" would also flip on those.
//!
//! Changing the input source forgets the inference; until a keystroke is
//! observed the platform's own answer stands.

use agenterm_platform::{
    ime::ImeEvent,
    input::{KeyPressState, NormalizedKeyEvent, PhysicalKeyCode},
};

#[derive(Debug, Default)]
pub(crate) struct ImeModeInference {
    source: String,
    native: Option<bool>,
    /// A composition was shown since the last letter key, so a commit that
    /// follows it is the end of a composition, not plain latin input.
    preedit_since_key: bool,
}

impl ImeModeInference {
    /// The inferred mode, if a keystroke has been observed since the input
    /// source last changed.
    pub(crate) fn native(&self) -> Option<bool> {
        self.native
    }

    /// Forget the inference when the selected input source changes.
    pub(crate) fn observe_source(&mut self, name: &str) {
        if self.source != name {
            self.source = name.to_owned();
            self.native = None;
            self.preedit_since_key = false;
        }
    }

    /// Returns true when the inferred mode changed.
    pub(crate) fn observe_key(&mut self, key: &NormalizedKeyEvent) -> bool {
        let modifiers = key.modifiers;
        let letter = key.state == KeyPressState::Pressed
            && matches!(key.physical, PhysicalKeyCode::Letter(_))
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.meta;
        if !letter {
            return false;
        }
        self.preedit_since_key = false;
        // The key produced its own letter: the input method passed it through.
        if key
            .text
            .as_deref()
            .is_some_and(|text| !text.is_empty() && text.chars().all(|ch| ch.is_ascii_alphabetic()))
        {
            return self.set(false);
        }
        false
    }

    /// Returns true when the inferred mode changed.
    pub(crate) fn observe_ime(&mut self, event: &ImeEvent) -> bool {
        match event {
            ImeEvent::Preedit { text, .. } if !text.is_empty() => {
                self.preedit_since_key = true;
                self.set(true)
            }
            // A letter committed without ever composing: latin passthrough
            // delivered through the IME channel rather than as a key's text.
            ImeEvent::Commit(text)
                if !self.preedit_since_key
                    && !text.is_empty()
                    && text.chars().all(|ch| ch.is_ascii_alphabetic()) =>
            {
                self.set(false)
            }
            _ => false,
        }
    }

    fn set(&mut self, native: bool) -> bool {
        let changed = self.native != Some(native);
        self.native = Some(native);
        changed
    }
}

#[cfg(test)]
mod tests {
    use agenterm_platform::input::{LogicalKey, ModifierState};

    use super::*;

    fn key(
        physical: PhysicalKeyCode,
        text: Option<&str>,
        state: KeyPressState,
        shift: bool,
    ) -> NormalizedKeyEvent {
        NormalizedKeyEvent {
            logical: match text {
                Some(text) => LogicalKey::Character(text.to_owned()),
                None => LogicalKey::Unidentified,
            },
            physical,
            text: text.map(str::to_owned),
            state,
            repeat: false,
            modifiers: ModifierState {
                shift,
                ..ModifierState::empty()
            },
        }
    }

    #[test]
    fn a_composition_means_native_and_passthrough_means_latin() {
        let mut inference = ImeModeInference::default();
        inference.observe_source("微信输入法");
        assert_eq!(inference.native(), None, "nothing observed yet");

        // Native: the letter's own text is withheld and a preedit appears.
        inference.observe_key(&key(
            PhysicalKeyCode::Letter('n'),
            None,
            KeyPressState::Pressed,
            false,
        ));
        assert!(inference.observe_ime(&ImeEvent::Preedit {
            text: "n".into(),
            cursor: None
        }));
        assert_eq!(inference.native(), Some(true));
        // Its commit is Chinese and ends the composition; still native.
        assert!(!inference.observe_ime(&ImeEvent::Commit("你".into())));
        assert_eq!(inference.native(), Some(true));

        // Latin: the letter arrives as plain text.
        assert!(inference.observe_key(&key(
            PhysicalKeyCode::Letter('a'),
            Some("a"),
            KeyPressState::Pressed,
            false
        )));
        assert_eq!(inference.native(), Some(false));
    }

    #[test]
    fn a_latin_letter_committed_without_composing_means_latin() {
        let mut inference = ImeModeInference::default();
        inference.observe_source("搜狗拼音");
        inference.observe_key(&key(
            PhysicalKeyCode::Letter('h'),
            None,
            KeyPressState::Pressed,
            false,
        ));
        assert!(inference.observe_ime(&ImeEvent::Commit("h".into())));
        assert_eq!(inference.native(), Some(false));
    }

    /// A capital typed with Shift is still a passthrough letter.
    #[test]
    fn a_shifted_capital_passed_through_still_means_latin() {
        let mut inference = ImeModeInference::default();
        inference.observe_source("微信输入法");
        inference.observe_ime(&ImeEvent::Preedit {
            text: "a".into(),
            cursor: None,
        });
        assert!(inference.observe_key(&key(
            PhysicalKeyCode::Letter('a'),
            Some("A"),
            KeyPressState::Pressed,
            true
        )));
        assert_eq!(inference.native(), Some(false));
    }

    #[test]
    fn changing_the_input_source_forgets_the_inference() {
        let mut inference = ImeModeInference::default();
        inference.observe_source("微信输入法");
        inference.observe_ime(&ImeEvent::Preedit {
            text: "a".into(),
            cursor: None,
        });
        inference.observe_source("ABC");
        assert_eq!(inference.native(), None);
    }
}
