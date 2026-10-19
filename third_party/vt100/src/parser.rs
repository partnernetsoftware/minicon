/// A parser for terminal output which produces an in-memory representation of
/// the terminal contents.
pub struct Parser<CB: crate::callbacks::Callbacks = ()> {
    parser: vte::Parser,
    screen: crate::perform::WrappedScreen<CB>,
}

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

    /// Discard only unfinished input grammar, preserving cells, cursor,
    /// drawing attributes, terminal modes and callback state.
    pub fn reset_input(&mut self) {
        self.parser = vte::Parser::new();
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
