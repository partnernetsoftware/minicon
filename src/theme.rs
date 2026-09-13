//! Host-chrome color themes.
//!
//! The theme paints only MiniCon's own shell — sidebar, tab column, composer,
//! borders, and status color. Terminal body text keeps the xterm 256 palette in
//! [`crate::palette`] regardless of theme, so a program's output never changes
//! meaning when the user switches how the frame looks.

use crate::palette::Rgb;

/// The theme a user has selected. `Neutral` is the historical monochrome look
/// and stays the default so an upgrade changes nothing until asked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeChoice {
    /// Neutral Ink — the high-contrast monochrome host UI MiniCon shipped with.
    #[default]
    Neutral,
    /// Docs Ink — the blue-ink palette shared with the project website.
    Docs,
    /// Paper Ink — a light palette in the same hues as Docs Ink.
    Paper,
}

impl ThemeChoice {
    /// Stable identifier for the control surface, so automation can read and
    /// assert the theme without matching a display label.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Docs => "docs",
            Self::Paper => "paper",
        }
    }

    /// The next theme in the cycle, for the keyboard toggle that stands in for
    /// the settings panel's swatch row until that panel exists.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Neutral => Self::Docs,
            Self::Docs => Self::Paper,
            Self::Paper => Self::Neutral,
        }
    }
}

/// Concrete chrome colors for one theme. Field names are semantic roles, not
/// palette-specific names, so all three themes fill the same slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    /// Sidebar / tab-column background.
    pub sidebar_bg: Rgb,
    /// The canvas behind the empty workspace and around the terminal.
    pub canvas_bg: Rgb,
    /// Selected tab row background.
    pub surface: Rgb,
    /// Composer (input area) background.
    pub composer_bg: Rgb,
    /// Floating panel (help / settings) background.
    pub panel_bg: Rgb,
    /// Dividers, panel borders, and rules.
    pub border: Rgb,
    /// Tree branch markers.
    pub branch: Rgb,
    /// Primary chrome text.
    pub text: Rgb,
    /// Secondary chrome text.
    pub muted: Rgb,
    /// Primary accent: the new-terminal button, the active-tab bar, carets.
    pub accent: Rgb,
    /// Error and host-notice color.
    pub error: Rgb,
}

impl Theme {
    /// Resolve the concrete colors for a choice.
    #[must_use]
    pub const fn for_choice(choice: ThemeChoice) -> Self {
        match choice {
            // = src/main.rs historical values; `border` unifies the two paint
            // paths' drifted rules to the study's canonical 2a value.
            ThemeChoice::Neutral => Self {
                sidebar_bg: Rgb(0x08, 0x08, 0x08),
                canvas_bg: Rgb(0x00, 0x00, 0x00),
                surface: Rgb(0x32, 0x32, 0x32),
                composer_bg: Rgb(0x00, 0x00, 0x00),
                panel_bg: Rgb(0x14, 0x14, 0x14),
                border: Rgb(0x48, 0x48, 0x48),
                branch: Rgb(0x98, 0x98, 0x98),
                text: Rgb(0xF5, 0xF5, 0xF5),
                muted: Rgb(0xC0, 0xC0, 0xC0),
                accent: Rgb(0xFF, 0xFF, 0xFF),
                error: Rgb(0xFF, 0x5C, 0x5C),
            },
            // = docs/index.html brand variables (Turn 2, 2b Docs Ink).
            ThemeChoice::Docs => Self {
                sidebar_bg: Rgb(0x0b, 0x10, 0x16),
                canvas_bg: Rgb(0x07, 0x0b, 0x10),
                surface: Rgb(0x17, 0x21, 0x2c),
                composer_bg: Rgb(0x11, 0x18, 0x21),
                panel_bg: Rgb(0x11, 0x18, 0x21),
                border: Rgb(0x2b, 0x39, 0x48),
                branch: Rgb(0x9b, 0xaa, 0xb8),
                text: Rgb(0xee, 0xf4, 0xf8),
                muted: Rgb(0x9b, 0xaa, 0xb8),
                accent: Rgb(0x68, 0xd8, 0xf0),
                error: Rgb(0xf2, 0x77, 0x6b),
            },
            // = same hues, light (Turn 2, 2c Paper Ink).
            ThemeChoice::Paper => Self {
                sidebar_bg: Rgb(0xf3, 0xf5, 0xf7),
                canvas_bg: Rgb(0xff, 0xff, 0xff),
                surface: Rgb(0xe4, 0xe9, 0xee),
                composer_bg: Rgb(0xe4, 0xe9, 0xee),
                panel_bg: Rgb(0xe4, 0xe9, 0xee),
                border: Rgb(0xc7, 0xd0, 0xd9),
                branch: Rgb(0x5b, 0x6a, 0x77),
                text: Rgb(0x0b, 0x10, 0x16),
                muted: Rgb(0x5b, 0x6a, 0x77),
                accent: Rgb(0x1f, 0x6f, 0x86),
                error: Rgb(0xb0, 0x3a, 0x2e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_the_default_and_keeps_the_historical_values() {
        assert_eq!(ThemeChoice::default(), ThemeChoice::Neutral);
        let t = Theme::for_choice(ThemeChoice::Neutral);
        assert_eq!(t.sidebar_bg, Rgb(0x08, 0x08, 0x08));
        assert_eq!(t.accent, Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(t.error, Rgb(0xFF, 0x5C, 0x5C));
    }

    #[test]
    fn every_choice_has_a_distinct_tag_and_the_cycle_returns_home() {
        let mut seen = std::collections::BTreeSet::new();
        let mut c = ThemeChoice::Neutral;
        for _ in 0..3 {
            assert!(seen.insert(c.tag()), "tags must be distinct");
            c = c.next();
        }
        assert_eq!(c, ThemeChoice::Neutral, "three steps return to the start");
    }

    #[test]
    fn light_and_dark_themes_actually_differ() {
        let neutral = Theme::for_choice(ThemeChoice::Neutral);
        let paper = Theme::for_choice(ThemeChoice::Paper);
        assert_ne!(neutral.canvas_bg, paper.canvas_bg);
        assert_ne!(neutral.text, paper.text);
    }
}
