//! OS/ISA-neutral terminal state and ordered output. No native process or
//! event-loop ownership is implied by this module.

mod utf16;
mod viewport;
pub use utf16::Utf16Decoder;
pub use viewport::{TerminalViewport, ViewportFollow};

use std::io::{self, Write};
pub use vt100::{
    Callbacks, Cell, CellMoveError, CellRect, CellStyle, Color, CursorShape, FrameError,
    MouseProtocolEncoding, MouseProtocolMode, RowRange, Screen, ScreenDamage, TEXT_BATCH_BYTES,
};

/// A parser paired with bounded output framing for its entire lifetime.
/// Screen edits cannot accidentally replace its unfinished client decoder.
/// Native projections must be complete commands and must update the shared
/// screen only after successful publication to the same sink.
pub struct TerminalStream<CB: Callbacks = ()> {
    parser: vt100::Parser<CB>,
    framer: vt100::OutputFramer,
}

impl TerminalStream {
    /// Construct a terminal without callbacks; sizes are rows then columns.
    #[must_use]
    pub fn new(rows: u16, columns: u16, scrollback: usize, pending_limit: usize) -> Self {
        Self::with_callbacks(rows, columns, scrollback, pending_limit, ())
    }
}

impl<CB: Callbacks> TerminalStream<CB> {
    /// Construct a terminal with caller-owned event callbacks. The pending
    /// limit bounds unfinished VT/UTF-8 bytes, not cells or scrollback memory.
    #[must_use]
    pub fn with_callbacks(
        rows: u16,
        columns: u16,
        scrollback: usize,
        pending_limit: usize,
        callbacks: CB,
    ) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, columns, scrollback, callbacks),
            framer: vt100::OutputFramer::new(pending_limit),
        }
    }

    /// Feed client bytes, publishing complete frames to the downstream sink.
    /// Earlier complete frames may be committed before a later error. Retire
    /// a transport after partial sink failure; rollback is not guaranteed.
    pub fn write_output(&mut self, bytes: &[u8], sink: &mut impl Write) -> Result<(), FrameError> {
        self.framer.write(&mut self.parser, bytes, sink)
    }

    /// Publish a complete screen command using an independent transient
    /// decoder. Client grammar and event callbacks are preserved. Projection
    /// events are not delivered to client callbacks; this path is intended for
    /// generated cursor, style and mode commands rather than client output.
    /// An unfinished command is rejected before publication. Commit the
    /// resulting screen only after the sink accepts the entire projection.
    pub fn write_projection(&mut self, bytes: &[u8], sink: &mut impl Write) -> io::Result<()> {
        let mut projection = vt100::Parser::new(1, 1, 0);
        *projection.screen_mut() = self.screen().clone();
        projection.process(bytes);
        if !projection.at_boundary() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unfinished terminal screen projection",
            ));
        }
        sink.write_all(bytes)?;
        let (screen, ()) = projection.into_parts();
        *self.parser.screen_mut() = screen;
        Ok(())
    }

    /// Whether no client VT command or UTF-8 scalar is unfinished.
    #[must_use]
    pub fn at_boundary(&self) -> bool {
        self.parser.at_boundary()
    }

    /// Inspect a bounded complete UTF-8 drawing prefix without changing the
    /// screen, decoder or callbacks. `newlines` permits visual CR/LF.
    #[must_use]
    pub fn text_prefix(&self, bytes: &[u8], newlines: bool) -> usize {
        self.framer.text_prefix(&self.parser, bytes, newlines)
    }

    /// Current terminal cells, cursor and modes.
    #[must_use]
    pub fn screen(&self) -> &Screen {
        self.parser.screen()
    }

    /// Edit cells, styles or dimensions without touching client input grammar.
    pub fn screen_mut(&mut self) -> &mut Screen {
        self.parser.screen_mut()
    }

    /// Consume accumulated conservative damage.
    pub fn take_damage(&mut self) -> ScreenDamage {
        self.parser.take_damage()
    }

    /// Inspect event callbacks without exposing mutable decoder state.
    #[must_use]
    pub fn callbacks(&self) -> &CB {
        self.parser.callbacks()
    }

    /// Bytes accepted but withheld until a complete grammar boundary.
    #[must_use]
    pub fn pending_bytes(&self) -> &[u8] {
        self.framer.pending_bytes()
    }

    /// Consume the stream after all producers and native projections stop,
    /// publishing any accepted incomplete tail. On failure the stream remains
    /// consumed: the caller must retire the transport rather than replay it.
    pub fn finish(mut self, sink: &mut impl Write) -> io::Result<(Screen, CB)> {
        self.flush_final_tail(sink)?;
        Ok(self.parser.into_parts())
    }

    // Native host finalization retains its existing private-Job authority.
    // Keep this borrowed operation crate-private; public owners consume finish.
    pub(crate) fn flush_final_tail(&mut self, sink: &mut impl Write) -> io::Result<()> {
        self.framer.finish(sink)
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalStream;

    #[test]
    fn rust_console_terminal_projection_is_transactional_and_preserves_client_events() {
        #[derive(Default)]
        struct Bells(usize);
        impl super::Callbacks for Bells {
            fn audible_bell(&mut self, _: &mut super::Screen) {
                self.0 += 1;
            }
        }
        let mut terminal = TerminalStream::with_callbacks(3, 8, 0, 64, Bells::default());
        let mut wire = Vec::new();
        terminal.write_output(b"A\x07\x1b[3", &mut wire).unwrap();
        let before = terminal.screen().state_formatted();
        let command = b"\x1b[2;3H\x1b[32m";
        struct Reject;
        impl std::io::Write for Reject {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reject before writing"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(terminal.write_projection(command, &mut Reject).is_err());
        assert_eq!(terminal.screen().state_formatted(), before);
        assert_eq!(terminal.pending_bytes(), b"\x1b[3");
        assert!(!terminal.at_boundary());
        let error = terminal
            .write_projection(b"\x1b[2;", &mut wire)
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(wire, b"A\x07");
        terminal.write_projection(command, &mut wire).unwrap();
        assert_eq!(terminal.callbacks().0, 1);
        assert_eq!(terminal.pending_bytes(), b"\x1b[3");
        terminal.write_output(b"1mB\x07", &mut wire).unwrap();
        assert!(terminal.at_boundary());
        assert_eq!(terminal.callbacks().0, 2);
        let mut consumer = vt100::Parser::new(3, 8, 0);
        consumer.process(&wire);
        assert_eq!(terminal.screen().contents(), consumer.screen().contents());
        assert_eq!(terminal.screen().cell(1, 2).unwrap().contents(), "B");
        assert_eq!(
            terminal.screen().cell(1, 2).unwrap().fgcolor(),
            super::Color::Idx(1)
        );
        let (_, callbacks) = terminal.finish(&mut wire).unwrap();
        assert_eq!(callbacks.0, 2);
    }

    #[test]
    fn rust_console_terminal_stream_preserves_pending_grammar_across_screen_edits() {
        let mut terminal = TerminalStream::new(2, 8, 0, 64);
        let mut wire = Vec::new();
        terminal.write_output(b"A\x1b[3", &mut wire).unwrap();
        assert_eq!(wire, b"A");
        terminal.screen_mut().fill_cell_text(1, 0, 1, 'N');
        terminal.write_output(b"1mB", &mut wire).unwrap();
        assert_eq!(
            terminal.screen().cell(0, 1).unwrap().fgcolor(),
            super::Color::Idx(1)
        );
        assert_eq!(terminal.screen().cell(1, 0).unwrap().contents(), "N");
        terminal.write_output(&[0xe4, 0xb8], &mut wire).unwrap();
        assert_eq!(terminal.pending_bytes(), &[0xe4, 0xb8]);
        let (screen, ()) = terminal.finish(&mut wire).unwrap();
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "A");
        assert_eq!(wire, b"A\x1b[31mB\xe4\xb8");
    }
}
