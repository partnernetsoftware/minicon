/// A parser for terminal output which produces an in-memory representation of
/// the terminal contents.
pub struct Parser<CB: crate::callbacks::Callbacks = ()> {
    parser: vte::Parser,
    screen: crate::perform::WrappedScreen<CB>,
}

mod text;

impl Parser {
    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback.
    #[must_use]
    pub fn new(rows: u16, cols: u16, scrollback_len: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new(rows, cols, scrollback_len),
        }
    }
}

impl<CB: crate::callbacks::Callbacks> Parser<CB> {
    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback. Terminal events will be reported via method
    /// calls on the provided [`Callbacks`](crate::callbacks::Callbacks)
    /// implementation.
    pub fn new_with_callbacks(rows: u16, cols: u16, scrollback_len: usize, callbacks: CB) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new_with_callbacks(
                rows,
                cols,
                scrollback_len,
                callbacks,
            ),
        }
    }

    /// Processes the contents of the given byte string, and updates the
    /// in-memory terminal state.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    /// Whether no VT command or UTF-8 scalar is unfinished.
    #[must_use]
    pub fn at_boundary(&self) -> bool {
        self.parser.at_boundary()
    }

    /// Preview grammar progress without changing callbacks or screen state.
    #[must_use]
    pub fn preview_byte(&self, byte: u8) -> crate::ParseProgress {
        self.parser.preview_byte(byte)
    }

    /// Fork screen and exact lexer state with independent callbacks. Existing
    /// callbacks are neither cloned nor invoked. This supports transactional
    /// render adapters; the fork includes unfinished VT/UTF-8 input grammar.
    #[must_use]
    pub fn fork_with_callbacks<T: crate::Callbacks>(&self, callbacks: T) -> Parser<T> {
        Parser {
            parser: self.parser.clone(),
            screen: crate::perform::WrappedScreen {
                screen: self.screen.screen.clone(),
                callbacks,
            },
        }
    }

    /// Commit a fork's screen and exact grammar while retaining this parser's
    /// callback owner. Return the fork's callbacks, such as collected effects.
    /// The caller must publish its projection before committing a staged fork.
    pub fn commit_fork<T: crate::Callbacks>(&mut self, fork: Parser<T>) -> T {
        self.parser = fork.parser;
        self.screen.screen = fork.screen.screen;
        fork.screen.callbacks
    }

    /// Discard only unfinished input grammar, preserving cells, cursor,
    /// drawing attributes, terminal modes and callback state.
    pub fn reset_input(&mut self) {
        self.parser = vte::Parser::new();
    }

    // Stage canonical recovery with a fresh decoder. A publisher must retain
    // its usual sink-before-screen-commit contract; restore old grammar if it
    // rejects the publication. Screen/callback ownership is never replaced.
    pub(crate) fn with_ground_input<T, E>(
        &mut self,
        publish: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let old = std::mem::take(&mut self.parser);
        let result = publish(self);
        if result.is_err() {
            self.parser = old;
        }
        result
    }

    /// Consume the parser, returning its screen and callback state without
    /// copying the grid. Any unfinished input grammar is discarded.
    #[must_use]
    pub fn into_parts(self) -> (crate::Screen, CB) {
        (self.screen.screen, self.screen.callbacks)
    }

    /// Takes conservative damage accumulated by the terminal model.
    ///
    /// The value is reset by this call. A caller should take it after one
    /// parser batch and before starting the next batch.
    pub fn take_damage(&mut self) -> crate::ScreenDamage {
        self.screen.screen.take_damage()
    }

    /// Returns a reference to a [`Screen`](crate::Screen) object containing
    /// the terminal state.
    #[must_use]
    pub fn screen(&self) -> &crate::Screen {
        &self.screen.screen
    }

    /// Returns a mutable reference to a [`Screen`](crate::Screen) object
    /// containing the terminal state.
    #[must_use]
    pub fn screen_mut(&mut self) -> &mut crate::Screen {
        &mut self.screen.screen
    }

    /// Returns a reference to the [`Callbacks`](crate::callbacks::Callbacks)
    /// state object passed into the constructor.
    pub fn callbacks(&self) -> &CB {
        &self.screen.callbacks
    }

    /// Returns a mutable reference to the
    /// [`Callbacks`](crate::callbacks::Callbacks) state object passed into
    /// the constructor.
    pub fn callbacks_mut(&mut self) -> &mut CB {
        &mut self.screen.callbacks
    }
}

impl Default for Parser {
    /// Returns a parser with dimensions 80x24 and no scrollback.
    fn default() -> Self {
        Self::new(24, 80, 0)
    }
}

impl std::io::Write for Parser {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.process(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod boundary_tests {
    #[test]
    fn replacement_scalar_stream_recovery_and_replay_are_lossless() {
        #[derive(Default)]
        struct Events(Vec<char>, Vec<u8>);
        impl crate::Callbacks for Events {
            fn unhandled_char(&mut self, _: &mut crate::Screen, character: char) {
                self.0.push(character);
            }
            fn unhandled_control(&mut self, _: &mut crate::Screen, byte: u8) {
                self.1.push(byte);
            }
        }
        let mut parser = super::Parser::new_with_callbacks(2, 12, 0, Events::default());
        parser.take_damage();
        parser.process(&[0xef, 0xbf]);
        assert_eq!(parser.screen().cursor_position(), (0, 0));
        parser.process(&[0xbd]);
        assert_eq!(parser.screen().cell(0, 0).unwrap().contents(), "\u{fffd}");
        assert!(parser.take_damage().rows().contains(0));
        parser.process(&[0xe4]);
        parser.process(b"A\xffB");
        assert_eq!(parser.screen().contents(), "\u{fffd}\u{fffd}A\u{fffd}B");
        assert_eq!(parser.screen().cursor_position(), (0, 5));
        assert!(parser.callbacks().0.is_empty());
        parser.process("\u{85}".as_bytes());
        assert!(parser.callbacks().0.is_empty());
        assert_eq!(parser.callbacks().1, [0x85]);
        let before = parser.screen().clone();
        parser.screen_mut().fill_cell_text(1, 1, 1, '\u{fffd}');
        for projection in [
            parser.screen().state_formatted(),
            parser.screen().state_diff(&before),
        ] {
            let mut replay = super::Parser::new(2, 12, 0);
            replay.process(&before.state_formatted());
            replay.process(&projection);
            assert_eq!(replay.screen().contents(), parser.screen().contents());
            assert_eq!(replay.screen().cell(1, 1).unwrap().contents(), "\u{fffd}");
            assert_eq!(replay.screen().cursor_position(), (0, 5));
        }
    }

    #[test]
    fn stream_boundary_reset_preserves_shared_screen_and_edit_state() {
        let mut parser = super::Parser::new(2, 8, 0);
        parser.process(b"AB\x1b[3");
        assert!(!parser.at_boundary());
        assert!(!parser.preview_byte(b'1').at_boundary);
        parser.screen_mut().fill_cell_text(1, 1, 1, 'N');
        parser.process(b"1mC");
        assert!(parser.at_boundary());
        assert_eq!(
            parser.screen().cell(0, 2).unwrap().fgcolor(),
            crate::Color::Idx(1)
        );
        parser.process(&[0xe4, 0xb8]);
        assert!(!parser.at_boundary());
        let before = parser.screen().clone();
        parser.reset_input();
        assert!(parser.at_boundary());
        assert_eq!(parser.screen().state_formatted(), before.state_formatted());
        parser.process(b"X");
        assert_eq!(parser.screen().cell(0, 3).unwrap().contents(), "X");
        assert_eq!(parser.screen().cell(1, 1).unwrap().contents(), "N");
        let (screen, ()) = parser.into_parts();
        assert_eq!(screen.cell(0, 3).unwrap().contents(), "X");
    }
}
