//! OS/ISA-neutral terminal state and ordered output. No native process or
//! event-loop ownership is implied by this module.

mod utf16;
mod viewport;
pub use utf16::Utf16Decoder;
pub use viewport::{ProjectionLimit, TerminalViewport, ViewportFollow};

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
        let mut parser = vt100::Parser::new_with_callbacks(rows, columns, scrollback, callbacks);
        parser.set_window_title_tracking(true);
        Self {
            parser,
            framer: vt100::OutputFramer::new(pending_limit),
        }
    }

    /// Feed client bytes, publishing complete frames to the downstream sink.
    /// Earlier complete frames may be committed before a later error. Retire
    /// a transport after partial sink failure; rollback is not guaranteed.
    pub fn write_output(&mut self, bytes: &[u8], sink: &mut impl Write) -> Result<(), FrameError> {
        self.framer.write(&mut self.parser, bytes, sink)
    }

    /// Publish a complete window/icon title independently of client grammar
    /// and viewport rendering. The caller owns title metadata. Control
    /// characters are rejected before publication; native adapters must apply
    /// their own display conversion first. A partial sink failure requires
    /// retiring the transport, as with other terminal output.
    pub fn write_window_title(
        &self,
        title: &str,
        limit: usize,
        sink: &mut impl Write,
    ) -> io::Result<()> {
        if title.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "control character in terminal title",
            ));
        }
        let requested = title.len().saturating_add(6);
        if requested > limit {
            return Err(io::Error::new(
                io::ErrorKind::QuotaExceeded,
                ProjectionLimit { requested, limit },
            ));
        }
        let mut wire = Vec::with_capacity(requested);
        wire.extend_from_slice(b"\x1b]0;");
        wire.extend_from_slice(title.as_bytes());
        wire.extend_from_slice(b"\x1b\\");
        sink.write_all(&wire)
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

    /// Take the latest committed OSC 0/2 title event. A later icon-only event,
    /// native projection or screen reset does not replace it. Events coalesce;
    /// use callbacks when every intermediate event is required.
    pub fn take_window_title_update(&mut self) -> Option<Vec<u8>> {
        self.parser.take_window_title_update()
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
    fn rust_console_terminal_osc_cancellation_preserves_committed_metadata_and_retry() {
        use super::{FrameError, TerminalViewport};
        fn feed(
            stream: &mut TerminalStream,
            view: &mut TerminalViewport,
            hidden: bool,
            data: &[u8],
            sink: &mut impl std::io::Write,
        ) -> Result<(), FrameError> {
            if hidden {
                stream.write_output_viewport_hidden(data, view, false, 4096, sink)
            } else {
                stream.write_output(data, sink)
            }
        }
        struct Reject;
        impl std::io::Write for Reject {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for hidden in [false, true] {
            let mut stream = TerminalStream::new(4, 16, 0, 128);
            let mut view = TerminalViewport::full(4, 16);
            let mut wire = Vec::new();
            feed(
                &mut stream,
                &mut view,
                hidden,
                b"\x1b]2;anchor\x07",
                &mut wire,
            )
            .unwrap();
            assert_eq!(stream.take_window_title_update().unwrap(), b"anchor");
            wire.clear();
            for data in [
                b"\x1b]2;bad\x18".as_slice(),
                b"\x1b]2;bad\x1a",
                b"\x1b]2;bad\x1b[31m",
                b"\x1b]2;bad\x1b\x1b\\",
            ] {
                feed(&mut stream, &mut view, hidden, data, &mut wire).unwrap();
                assert!(stream.at_boundary());
                assert_eq!(stream.take_window_title_update(), None);
                assert!(stream.screen().contents().is_empty());
            }
            if hidden {
                assert!(wire.is_empty());
            }
            feed(
                &mut stream,
                &mut view,
                hidden,
                b"\x1b]2;next\xc2",
                &mut wire,
            )
            .unwrap();
            assert_eq!(stream.take_window_title_update(), None);
            let pending = stream.pending_bytes().to_vec();
            let before = stream.screen().state_formatted();
            assert!(feed(&mut stream, &mut view, hidden, &[0x9c], &mut Reject).is_err());
            assert_eq!(stream.take_window_title_update(), None);
            assert_eq!(stream.pending_bytes(), pending);
            assert_eq!(stream.screen().state_formatted(), before);
            feed(&mut stream, &mut view, hidden, &[0x9c], &mut wire).unwrap();
            assert_eq!(stream.take_window_title_update().unwrap(), b"next");
            assert!(stream.at_boundary());
            feed(
                &mut stream,
                &mut view,
                hidden,
                b"\x1b]2;fresh\x1b",
                &mut wire,
            )
            .unwrap();
            assert_eq!(stream.take_window_title_update(), None);
            feed(&mut stream, &mut view, hidden, b"\\", &mut wire).unwrap();
            assert_eq!(stream.take_window_title_update().unwrap(), b"fresh");
        }
    }

    #[test]
    fn rust_console_terminal_title_events_commit_complete_frames_and_survive_screen_reset() {
        let mut direct = vt100::Parser::new(1, 8, 0);
        direct.process(b"\x1b]2;untracked\x07");
        assert_eq!(direct.take_window_title_update(), None);
        direct.set_window_title_tracking(true);
        direct.process(b"\x1b]2;tracked\x07");
        direct.set_window_title_tracking(false);
        assert_eq!(direct.take_window_title_update(), None);
        assert!(direct.at_boundary());
        #[derive(Default)]
        struct Events {
            titles: Vec<Vec<u8>>,
            icons: usize,
        }
        impl super::Callbacks for Events {
            fn set_window_title(&mut self, _: &mut super::Screen, title: &[u8]) {
                self.titles.push(title.to_vec());
            }
            fn set_window_icon_name(&mut self, _: &mut super::Screen, _: &[u8]) {
                self.icons += 1;
            }
        }
        struct Reject;
        impl std::io::Write for Reject {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut stream = TerminalStream::with_callbacks(4, 16, 0, 128, Events::default());
        let title = format!("vt;{}中😀", "x;".repeat(24));
        let prefix = format!("\x1b]2;{title}\x1b");
        let mut wire = Vec::new();
        stream.write_output(prefix.as_bytes(), &mut wire).unwrap();
        assert!(wire.is_empty());
        assert!(stream.callbacks().titles.is_empty());
        assert_eq!(stream.take_window_title_update(), None);
        stream.write_projection(b"\x1b[2;3HN", &mut wire).unwrap();
        assert!(stream.write_output(b"\\", &mut Reject).is_err());
        assert!(stream.callbacks().titles.is_empty());
        assert_eq!(stream.take_window_title_update(), None);
        assert_eq!(stream.pending_bytes(), prefix.as_bytes());
        stream.write_output(b"\\", &mut wire).unwrap();
        assert_eq!(stream.take_window_title_update().unwrap(), title.as_bytes());
        assert_eq!(stream.take_window_title_update(), None);
        assert_eq!(stream.callbacks().titles, vec![title.as_bytes().to_vec()]);
        stream
            .write_output(b"\x1b]2;last\x07\x1b]1;icon;only\x07\x1bc", &mut wire)
            .unwrap();
        assert_eq!(stream.take_window_title_update().unwrap(), b"last");
        assert_eq!(stream.callbacks().icons, 1);
        assert_eq!(stream.callbacks().titles.len(), 2);
        assert!(stream.screen().contents().is_empty());
        let bytes = [b"\x1b]2;before-limit\x07\x1b]2;".as_slice(), &[b'x'; 129]].concat();
        assert!(matches!(
            stream.write_output(&bytes, &mut wire),
            Err(super::FrameError::PendingLimit)
        ));
        assert_eq!(stream.take_window_title_update().unwrap(), b"before-limit");
        assert_eq!(stream.callbacks().titles.len(), 3);
    }

    #[test]
    fn rust_console_terminal_title_publication_preserves_grammar_and_rejects_injection() {
        let mut terminal = TerminalStream::new(2, 16, 0, 64);
        let mut wire = Vec::new();
        terminal.write_output(b"ROOT\x1b[3", &mut wire).unwrap();
        let before = terminal.screen().state_formatted();
        let mut titles = Vec::new();
        terminal
            .write_window_title("native-中😀;tail", 128, &mut titles)
            .unwrap();
        assert_eq!(titles, "\x1b]0;native-中😀;tail\x1b\\".as_bytes());
        for title in ["A\x07BAD", "A\x1b]2;BAD", "A\u{009c}BAD", "A\x7fBAD"] {
            let mut rejected = Vec::new();
            assert_eq!(
                terminal
                    .write_window_title(title, 128, &mut rejected)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert!(rejected.is_empty());
        }
        let mut rejected = Vec::new();
        let error = terminal
            .write_window_title("native", 11, &mut rejected)
            .unwrap_err();
        assert_eq!(
            super::ProjectionLimit::from_error(&error),
            Some(&super::ProjectionLimit {
                requested: 12,
                limit: 11
            })
        );
        assert!(rejected.is_empty());
        struct Reject;
        impl std::io::Write for Reject {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reject title"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(
            terminal
                .write_window_title("native", 12, &mut Reject)
                .is_err()
        );
        terminal.write_window_title("", 6, &mut titles).unwrap();
        assert!(titles.ends_with(b"\x1b]0;\x1b\\"));
        assert_eq!(terminal.pending_bytes(), b"\x1b[3");
        assert_eq!(terminal.screen().state_formatted(), before);
        terminal.write_output(b"1mZ", &mut wire).unwrap();
        assert_eq!(terminal.screen().contents(), "ROOTZ");
        assert_eq!(
            terminal.screen().cell(0, 4).unwrap().fgcolor(),
            super::Color::Idx(1)
        );
        terminal.write_output(&[0xe4, 0xb8], &mut wire).unwrap();
        terminal
            .write_window_title("UTF8", 10, &mut titles)
            .unwrap();
        assert_eq!(terminal.pending_bytes(), &[0xe4, 0xb8]);
        terminal.write_output(&[0xad], &mut wire).unwrap();
        assert_eq!(terminal.screen().contents(), "ROOTZ中");
    }

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
