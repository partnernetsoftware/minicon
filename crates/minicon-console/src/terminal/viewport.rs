//! Stateless viewport rendering from one authoritative drawing grid.
use super::{Callbacks, CellRect, FrameError, Screen, TerminalStream};
use std::io::{self, Write};

/// A generated terminal projection exceeded its caller-owned byte budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionLimit {
    /// Encoded size of the proposed projection.
    pub requested: usize,
    /// Maximum permitted encoded size.
    pub limit: usize,
}

impl ProjectionLimit {
    /// Identify a projection-budget failure without interpreting OS codes.
    #[must_use]
    pub fn from_error(error: &io::Error) -> Option<&Self> {
        error.get_ref()?.downcast_ref()
    }
}

impl std::fmt::Display for ProjectionLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "terminal projection requires {} bytes; limit is {}",
            self.requested, self.limit
        )
    }
}
impl std::error::Error for ProjectionLimit {}

/// Cursor-driven viewport movement, independent of native output modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportFollow {
    /// Keep the supplied source region fixed.
    Fixed,
    /// Follow cursor rows while preserving the horizontal source region.
    Vertical,
    /// Follow both cursor coordinates.
    Cursor,
}

impl From<bool> for ViewportFollow {
    fn from(follow: bool) -> Self {
        if follow { Self::Cursor } else { Self::Fixed }
    }
}

impl ViewportFollow {
    fn apply(self, view: TerminalViewport, screen: &Screen) -> TerminalViewport {
        match self {
            Self::Fixed => view,
            Self::Cursor => view.following_cursor(screen),
            Self::Vertical => {
                let mut proposed = view.following_cursor(screen);
                proposed.area.left = view.area.left;
                proposed.area.right = view.area.right;
                proposed
            }
        }
    }
}

/// A source region and independently sized downstream terminal surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalViewport {
    /// Signed half-open source region.
    pub area: CellRect,
    /// Downstream rows, independent of source buffer size.
    pub rows: u16,
    /// Downstream columns, independent of source buffer size.
    pub columns: u16,
}

impl TerminalViewport {
    /// A viewport initially covering the entire downstream surface.
    #[must_use]
    pub fn full(rows: u16, columns: u16) -> Self {
        Self {
            area: CellRect {
                top: 0,
                left: 0,
                bottom: i32::from(rows),
                right: i32::from(columns),
            },
            rows,
            columns,
        }
    }

    fn snapshot(self, screen: &Screen) -> io::Result<Screen> {
        screen
            .project_region(self.area, self.rows, self.columns)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid terminal viewport"))
    }

    /// Move a bounded source region to include the drawing cursor. The
    /// downstream size does not change. This is an explicit caller policy.
    #[must_use]
    pub fn following_cursor(self, screen: &Screen) -> Self {
        let (rows, columns) = screen.size();
        let (row, column) = screen.cursor_position();
        let height =
            (i64::from(self.area.bottom) - i64::from(self.area.top)).clamp(1, i64::from(rows));
        let width =
            (i64::from(self.area.right) - i64::from(self.area.left)).clamp(1, i64::from(columns));
        let row = i64::from(row.min(rows - 1));
        let column = i64::from(column.min(columns - 1));
        let top = i64::from(self.area.top)
            .min(row)
            .max(row - height + 1)
            .clamp(0, i64::from(rows) - height);
        let left = i64::from(self.area.left)
            .min(column)
            .max(column - width + 1)
            .clamp(0, i64::from(columns) - width);
        Self {
            area: CellRect {
                top: top as i32,
                left: left as i32,
                bottom: (top + height) as i32,
                right: (left + width) as i32,
            },
            ..self
        }
    }
}

#[derive(Default)]
struct Effects {
    forward: bool,
}
impl Callbacks for Effects {
    fn audible_bell(&mut self, _: &mut Screen) {
        self.forward = true;
    }
    fn visual_bell(&mut self, _: &mut Screen) {
        self.forward = true;
    }
    fn resize(&mut self, _: &mut Screen, _: (u16, u16)) {
        self.forward = true;
    }
    fn set_window_icon_name(&mut self, _: &mut Screen, _: &[u8]) {
        self.forward = true;
    }
    fn set_window_title(&mut self, _: &mut Screen, _: &[u8]) {
        self.forward = true;
    }
    fn copy_to_clipboard(&mut self, _: &mut Screen, _: &[u8], _: &[u8]) {
        self.forward = true;
    }
    fn paste_from_clipboard(&mut self, _: &mut Screen, _: &[u8]) {
        self.forward = true;
    }
    fn unhandled_char(&mut self, _: &mut Screen, _: char) {
        self.forward = true;
    }
    fn unhandled_control(&mut self, _: &mut Screen, _: u8) {
        self.forward = true;
    }
    fn unhandled_escape(&mut self, _: &mut Screen, _: Option<u8>, _: Option<u8>, _: u8) {
        self.forward = true;
    }
    fn unhandled_csi(
        &mut self,
        _: &mut Screen,
        _: Option<u8>,
        _: Option<u8>,
        _: &[&[u16]],
        _: char,
    ) {
        self.forward = true;
    }
    fn unhandled_osc(&mut self, _: &mut Screen, _: &[&[u8]]) {
        self.forward = true;
    }
}

fn check_projection(bytes: &[u8], limit: usize) -> io::Result<()> {
    if bytes.len() > limit {
        Err(io::Error::new(
            io::ErrorKind::QuotaExceeded,
            ProjectionLimit {
                requested: bytes.len(),
                limit,
            },
        ))
    } else {
        Ok(())
    }
}

impl TerminalStream {
    /// Parse client data in its drawing buffer and publish a clipped viewport,
    /// forwarding complete nonvisual/extension frames. Only the default
    /// callback owner is supported here. Screen/grammar/view commit follows
    /// sink success; earlier completed frames may already have committed.
    /// The peer must initially match the supplied viewport snapshot. Use a
    /// full commit to enter viewport mode from a raw stream.
    pub fn write_output_viewport(
        &mut self,
        bytes: &[u8],
        view: &mut TerminalViewport,
        follow: impl Into<ViewportFollow>,
        limit: usize,
        sink: &mut impl Write,
    ) -> Result<(), FrameError> {
        let follow = follow.into();
        self.framer.write_mapped(
            &mut self.parser,
            bytes,
            sink,
            false,
            |parser, raw, delta, output| {
                let mut candidate = parser.fork_with_callbacks(Effects::default());
                candidate.process(delta);
                let effects = candidate.callbacks().forward;
                let proposed = if !effects {
                    follow.apply(*view, candidate.screen())
                } else {
                    *view
                };
                let old = view.snapshot(parser.screen())?;
                let next = proposed.snapshot(candidate.screen())?;
                let alt =
                    candidate.screen().alternate_screen() != parser.screen().alternate_screen();
                let mut wire = Vec::new();
                if effects || alt {
                    wire.extend_from_slice(raw);
                }
                if alt {
                    wire.extend_from_slice(&next.state_formatted());
                } else {
                    wire.extend_from_slice(&next.state_diff(&old));
                }
                check_projection(&wire, limit)?;
                output.write_all(&wire)?;
                parser.commit_fork(candidate);
                *view = proposed;
                Ok(())
            },
        )
    }
}

impl<CB: Callbacks> TerminalStream<CB> {
    /// Replace the peer image and select this stream's logical VT layer.
    /// Source cells, callbacks and unfinished client grammar are untouched.
    /// The caller commits displayed-stream ownership only after success;
    /// a partial sink failure requires retiring the transport.
    /// `limit` bounds wire bytes; callers bound viewport dimensions separately.
    pub fn write_viewport_activation(
        &self,
        view: TerminalViewport,
        limit: usize,
        sink: &mut impl Write,
    ) -> io::Result<()> {
        let snapshot = view.snapshot(self.screen())?;
        let mut wire = b"\x1b[?1049l".to_vec();
        if self.screen().alternate_screen() {
            wire.extend_from_slice(b"\x1b[?1049h");
        }
        wire.extend_from_slice(b"\x1b[?6l\x1b[r");
        wire.extend_from_slice(&snapshot.state_formatted());
        check_projection(&wire, limit)?;
        sink.write_all(&wire)
    }

    /// Publish and commit a staged screen/view edit without changing client
    /// grammar or callbacks. Full publication neutralizes physical margins
    /// and origin mode; subsequent output must use viewport rendering.
    pub fn commit_viewport_screen(
        &mut self,
        next: Screen,
        view: &mut TerminalViewport,
        proposed: TerminalViewport,
        limit: usize,
        full: bool,
        sink: &mut impl Write,
    ) -> io::Result<()> {
        let projected = proposed.snapshot(&next)?;
        let wire = if full {
            let mut wire = b"\x1b[?6l\x1b[r".to_vec();
            wire.extend_from_slice(&projected.state_formatted());
            wire
        } else {
            projected.state_diff(&view.snapshot(self.screen())?)
        };
        check_projection(&wire, limit)?;
        sink.write_all(&wire)?;
        *self.parser.screen_mut() = next;
        *view = proposed;
        Ok(())
    }

    /// Execute a complete native command with an independent decoder, then
    /// publish its viewport. Client partial commands and callbacks survive.
    pub fn write_viewport_projection(
        &mut self,
        bytes: &[u8],
        view: &mut TerminalViewport,
        follow: impl Into<ViewportFollow>,
        limit: usize,
        sink: &mut impl Write,
    ) -> io::Result<()> {
        let mut candidate = vt100::Parser::new(1, 1, 0);
        *candidate.screen_mut() = self.screen().clone();
        candidate.process(bytes);
        if !candidate.at_boundary() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unfinished viewport command",
            ));
        }
        let follow = follow.into();
        let proposed = if candidate.screen().cursor_position() != self.screen().cursor_position() {
            follow.apply(*view, candidate.screen())
        } else {
            *view
        };
        let (next, ()) = candidate.into_parts();
        self.commit_viewport_screen(next, view, proposed, limit, false, sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_terminal_activation_preserves_layers_grammar_and_portable_limits() {
        let mut first = TerminalStream::new(4, 8, 0, 64);
        first
            .write_output("BASE\x1b[?1049h\x1b[H中A\x1b[3".as_bytes(), &mut Vec::new())
            .unwrap();
        let mut other = TerminalStream::new(4, 8, 0, 64);
        other.write_output(b"NORM", &mut Vec::new()).unwrap();
        let mut view = TerminalViewport::full(4, 8);
        let before = first.screen().state_formatted();
        let mut wire = Vec::new();
        first
            .write_viewport_activation(view, 4096, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(4, 8, 0);
        peer.process(&wire);
        assert!(peer.screen().alternate_screen());
        assert_eq!(peer.screen().contents(), "中A");
        assert_eq!(first.pending_bytes(), b"\x1b[3");
        let quota = other
            .write_viewport_activation(view, 0, &mut wire)
            .unwrap_err();
        assert_eq!(quota.kind(), io::ErrorKind::QuotaExceeded);
        assert!(quota.raw_os_error().is_none());
        let details = ProjectionLimit::from_error(&quota).unwrap();
        assert!(details.requested > 0);
        assert_eq!(details.limit, 0);
        let mut invalid = view;
        invalid.rows = 0;
        assert_eq!(
            other
                .write_viewport_activation(invalid, 4096, &mut wire)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::QuotaExceeded,
                    "caller sink fault",
                ))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let rejected = other
            .write_viewport_activation(view, 4096, &mut Reject)
            .unwrap_err();
        assert!(ProjectionLimit::from_error(&rejected).is_none());
        assert_eq!(first.screen().state_formatted(), before);
        assert_eq!(first.pending_bytes(), b"\x1b[3");
        other
            .write_viewport_activation(view, 4096, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(4, 8, 0);
        peer.process(&wire);
        assert!(!peer.screen().alternate_screen());
        assert_eq!(peer.screen().contents(), "NORM");
        first
            .write_viewport_activation(view, 4096, &mut wire)
            .unwrap();
        first
            .write_output_viewport(b"1mZ", &mut view, false, 4096, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(4, 8, 0);
        peer.process(&wire);
        assert!(peer.screen().alternate_screen());
        assert_eq!(peer.screen().contents(), "中AZ");
        assert_eq!(
            peer.screen().cell(0, 3).unwrap().fgcolor(),
            super::super::Color::Idx(1)
        );
        first
            .write_output_viewport(b"\x1b[?1049l", &mut view, false, 4096, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(4, 8, 0);
        peer.process(&wire);
        assert!(!peer.screen().alternate_screen());
        assert_eq!(peer.screen().contents(), "BASE");
        assert_eq!(peer.screen().cursor_position(), (0, 4));
        struct Bells(usize);
        impl Callbacks for Bells {
            fn audible_bell(&mut self, _: &mut Screen) {
                self.0 += 1;
            }
        }
        let mut captured = TerminalStream::with_callbacks(4, 8, 0, 64, Bells(0));
        captured
            .write_output(b"\x07CAP\x1b[", &mut Vec::new())
            .unwrap();
        captured
            .write_viewport_activation(view, 4096, &mut wire)
            .unwrap();
        assert_eq!(captured.callbacks().0, 1);
        assert_eq!(captured.pending_bytes(), b"\x1b[");
    }

    #[test]
    fn rust_console_terminal_vertical_follow_preserves_columns_and_sink_transactions() {
        let mut stream = TerminalStream::new(16, 24, 0, 64);
        let mut view = TerminalViewport::full(4, 12);
        let mut wire = Vec::new();
        stream
            .write_output_viewport(
                b"01234567890123456789",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!((view.area.top, view.area.left), (0, 0));
        stream
            .write_output_viewport(
                b"\r\nL01\r\nL02\r\nL03\r\nL04\r\nL05\r\n",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!(
            (
                view.area.top,
                view.area.bottom,
                view.area.left,
                view.area.right
            ),
            (3, 7, 0, 12)
        );
        let mut peer = vt100::Parser::new(4, 12, 0);
        peer.process(&wire);
        assert_eq!(
            peer.screen().rows(0, 12).collect::<Vec<_>>(),
            ["L03", "L04", "L05", ""]
        );
        assert_eq!(peer.screen().cursor_position(), (3, 0));
        let before = stream.screen().state_formatted();
        let old = view;
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(
            stream
                .write_output_viewport(
                    b"MORE\r\n",
                    &mut view,
                    ViewportFollow::Vertical,
                    4096,
                    &mut Reject
                )
                .is_err()
        );
        assert_eq!(stream.screen().state_formatted(), before);
        assert_eq!(view, old);
        assert!(
            stream
                .write_output_viewport(
                    b"MORE\r\n",
                    &mut view,
                    ViewportFollow::Vertical,
                    0,
                    &mut wire
                )
                .is_err()
        );
        assert_eq!(stream.screen().state_formatted(), before);
        assert_eq!(view, old);
        stream
            .write_viewport_projection(b"\x1b[7;19H", &mut view, true, 4096, &mut wire)
            .unwrap();
        assert_eq!((view.area.top, view.area.left), (3, 7));
        stream
            .write_output_viewport(
                b"\r\nNEXT\r\n",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!((view.area.top, view.area.left, view.area.right), (5, 7, 19));
        stream
            .write_output_viewport(
                b"\x1b]2;effect\x07",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!((view.area.top, view.area.left), (5, 7));
        stream
            .write_viewport_projection(b"\x1b[1;1H", &mut view, true, 4096, &mut wire)
            .unwrap();
        assert_eq!((view.area.top, view.area.left), (0, 0));
        stream
            .write_output_viewport(b"\n\n\n\n\n", &mut view, false, 4096, &mut wire)
            .unwrap();
        assert_eq!(view.area.top, 0);
        stream
            .write_output_viewport(
                b"\x1b]2;fixed-effect\x07",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!(
            view.area.top, 0,
            "nonvisual effects must not bring an offscreen cursor into view"
        );
        stream
            .write_output_viewport(b"X", &mut view, true, 4096, &mut wire)
            .unwrap();
        assert_eq!(view.area.top, 2);
        stream
            .write_output_viewport(
                b"\xe4\xb8",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!(stream.pending_bytes(), b"\xe4\xb8");
        stream
            .write_viewport_projection(
                b"\x1b[12;3H",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert_eq!((view.area.top, view.area.left), (8, 0));
        stream
            .write_output_viewport(
                b"\xad",
                &mut view,
                ViewportFollow::Vertical,
                4096,
                &mut wire,
            )
            .unwrap();
        assert!(stream.at_boundary());
        assert_eq!(stream.screen().cell(11, 2).unwrap().contents(), "中");
        let mut peer = vt100::Parser::new(4, 12, 0);
        peer.process(&wire);
        assert_eq!(peer.screen().cell(3, 2).unwrap().contents(), "中");
        assert!(peer.at_boundary());
    }

    #[test]
    fn rust_console_terminal_malformed_utf8_recovery_precedes_native_edits() {
        let mut terminal = TerminalStream::new(3, 16, 0, 64);
        let mut view = TerminalViewport::full(3, 8);
        let mut wire = Vec::new();
        terminal
            .write_output_viewport(&[0xe4, 0xb8], &mut view, false, 4096, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(&[0xe3], &mut view, false, 4096, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(3, 8, 0);
        peer.process(&wire);
        assert_eq!(
            peer.screen().contents(),
            "\u{fffd}",
            "replacement must be visible before the new scalar completes"
        );
        assert_eq!(terminal.pending_bytes(), &[0xe3]);
        terminal
            .write_viewport_projection(b"\x1b[2;3H", &mut view, false, 4096, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(&[0x81, 0x82], &mut view, false, 4096, &mut wire)
            .unwrap();
        let (screen, ()) = terminal.finish(&mut wire).unwrap();
        let mut peer = vt100::Parser::new(3, 8, 0);
        peer.process(&wire);
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "\u{fffd}");
        assert_eq!(screen.cell(1, 2).unwrap().contents(), "あ");
        assert_eq!(peer.screen().contents(), screen.contents());
        assert!(peer.at_boundary());

        for suffix in [
            &b"\xe3\x81\x82"[..],
            &b"\x1b[31mZ"[..],
            &b"\x07Z"[..],
            &b"\xffZ"[..],
            &b"A"[..],
        ] {
            let mut terminal = TerminalStream::new(3, 16, 0, 64);
            let mut view = TerminalViewport::full(3, 8);
            let mut wire = Vec::new();
            terminal
                .write_output_viewport(b"\xe4\xb8", &mut view, false, 4096, &mut wire)
                .unwrap();
            for &byte in suffix {
                terminal
                    .write_output_viewport(&[byte], &mut view, false, 4096, &mut wire)
                    .unwrap();
                let mut peer = vt100::Parser::new(3, 8, 0);
                peer.process(&wire);
                assert_eq!(
                    peer.screen().state_formatted(),
                    view.snapshot(terminal.screen()).unwrap().state_formatted()
                );
                assert!(peer.at_boundary());
            }
        }
        let mut terminal = TerminalStream::new(3, 16, 0, 64);
        let mut view = TerminalViewport::full(3, 8);
        let mut wire = Vec::new();
        terminal
            .write_output_viewport(b"\xe4\xb8", &mut view, false, 4096, &mut wire)
            .unwrap();
        let before = terminal.screen().state_formatted();
        assert!(
            terminal
                .write_output_viewport(b"\xe3", &mut view, false, 0, &mut wire)
                .is_err()
        );
        assert_eq!(terminal.screen().state_formatted(), before);
        assert_eq!(terminal.pending_bytes(), b"\xe4\xb8");
        terminal
            .write_output_viewport(b"\xe3", &mut view, false, 4096, &mut wire)
            .unwrap();
        let tail_start = wire.len();
        let (screen, ()) = terminal.finish(&mut wire).unwrap();
        assert_eq!(&wire[tail_start..], b"\xe3");
        let mut peer = vt100::Parser::new(3, 8, 0);
        peer.process(&wire);
        assert_eq!(screen.contents(), "\u{fffd}");
        assert_eq!(peer.screen().contents(), screen.contents());
    }

    #[test]
    fn rust_console_terminal_unicode_batches_preserve_effects_fragments_and_errors() {
        let mut terminal = TerminalStream::new(4, 128, 0, 64);
        let mut view = TerminalViewport::full(4, 64);
        let proposed = view;
        let mut wire = Vec::new();
        terminal
            .commit_viewport_screen(
                terminal.screen().clone(),
                &mut view,
                proposed,
                16384,
                true,
                &mut wire,
            )
            .unwrap();
        assert_eq!(terminal.text_prefix("中\r\nZ".as_bytes(), false), 3);
        assert_eq!(terminal.text_prefix("中\r\nZ".as_bytes(), true), 6);
        assert_eq!(terminal.text_prefix("\u{85}Z".as_bytes(), true), 0);
        assert_eq!(terminal.text_prefix(b"A\xffZ", true), 1);
        struct Count<'a>(&'a mut Vec<u8>, usize);
        impl Write for Count<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.1 += 1;
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let text = "Ae\u{301}中😀BC".repeat(16);
        let mut count = Count(&mut wire, 0);
        terminal
            .write_output_viewport(text.as_bytes(), &mut view, false, 16384, &mut count)
            .unwrap();
        assert_eq!(count.1, 1);
        terminal
            .write_output_viewport(
                "\x07Q\u{85}R\x1b]2;bulk-title\x07Z".as_bytes(),
                &mut view,
                false,
                16384,
                &mut wire,
            )
            .unwrap();
        terminal
            .write_output_viewport(&[0xe4, 0xb8], &mut view, false, 16384, &mut wire)
            .unwrap();
        assert_eq!(terminal.text_prefix(b"X", true), 0);
        terminal
            .write_viewport_projection(b"\x1b[3;2H", &mut view, false, 16384, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(&[0xad], &mut view, false, 16384, &mut wire)
            .unwrap();
        #[derive(Default)]
        struct Events {
            bell: usize,
            c1: usize,
            title: Vec<u8>,
            at_bell: String,
        }
        impl Callbacks for Events {
            fn audible_bell(&mut self, screen: &mut Screen) {
                self.bell += 1;
                self.at_bell = screen.rows(0, 64).next().unwrap();
            }
            fn unhandled_char(&mut self, _: &mut Screen, c: char) {
                assert_eq!(c, '\u{85}');
                self.c1 += 1;
            }
            fn unhandled_control(&mut self, _: &mut Screen, byte: u8) {
                assert_eq!(byte, 0x85);
                self.c1 += 1;
            }
            fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
                self.title = title.to_vec();
            }
        }
        let mut peer = vt100::Parser::new_with_callbacks(4, 64, 0, Events::default());
        peer.process(&wire);
        assert_eq!(peer.callbacks().bell, 1);
        assert_eq!(peer.callbacks().c1, 1);
        assert_eq!(peer.callbacks().title, b"bulk-title");
        assert_eq!(peer.callbacks().at_bell, "Ae\u{301}中😀BC".repeat(8));
        assert_eq!(
            peer.screen().rows(0, 64).next().unwrap(),
            "Ae\u{301}中😀BC".repeat(8)
        );
        assert_eq!(peer.screen().rows(0, 64).nth(1).unwrap(), "QRZ");
        assert_eq!(peer.screen().cell(2, 1).unwrap().contents(), "中");
        let before = terminal.screen().state_formatted();
        let old = view;
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(
            terminal
                .write_output_viewport("界".as_bytes(), &mut view, false, 16384, &mut Reject)
                .is_err()
        );
        assert_eq!(terminal.screen().state_formatted(), before);
        assert_eq!(view, old);
        assert!(
            terminal
                .write_output_viewport("界".as_bytes(), &mut view, false, 0, &mut wire)
                .is_err()
        );
        assert_eq!(terminal.screen().state_formatted(), before);
        assert_eq!(view, old);
    }

    #[test]
    fn rust_console_terminal_viewport_preserves_buffer_grammar_effects_and_commit() {
        let mut terminal = TerminalStream::new(2, 4, 0, 64);
        let mut view = TerminalViewport::full(2, 4);
        let mut next = terminal.screen().clone();
        next.set_size(4, 8);
        let mut wire = Vec::new();
        let proposed = view;
        terminal
            .commit_viewport_screen(next, &mut view, proposed, 4096, true, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(b"ABCDEFGHijkl", &mut view, false, 4096, &mut wire)
            .unwrap();
        assert_eq!(terminal.screen().rows(0, 8).next().unwrap(), "ABCDEFGH");
        assert_eq!(terminal.screen().cursor_position(), (1, 4));
        terminal
            .write_output_viewport(&[0xe4, 0xb8], &mut view, false, 4096, &mut wire)
            .unwrap();
        terminal
            .write_viewport_projection(b"\x1b[2;2H\x1b[32m", &mut view, false, 4096, &mut wire)
            .unwrap();
        assert_eq!(terminal.pending_bytes(), &[0xe4, 0xb8]);
        terminal
            .write_output_viewport(&[0xad], &mut view, false, 4096, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(b"\x1b[3\x071m", &mut view, false, 4096, &mut wire)
            .unwrap();
        terminal
            .write_output_viewport(
                b"\x1b]2;projected-title\x07",
                &mut view,
                false,
                4096,
                &mut wire,
            )
            .unwrap();
        terminal
            .write_output_viewport(
                b"\x1b[?1049hALT\x1b[?1049l",
                &mut view,
                false,
                4096,
                &mut wire,
            )
            .unwrap();
        #[derive(Default)]
        struct Events(usize, Vec<u8>);
        impl Callbacks for Events {
            fn audible_bell(&mut self, _: &mut Screen) {
                self.0 += 1;
            }
            fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
                self.1 = title.to_vec();
            }
        }
        let mut peer = vt100::Parser::new_with_callbacks(2, 4, 0, Events::default());
        peer.process(&wire);
        assert_eq!(peer.screen().rows(0, 4).next().unwrap(), "ABCD");
        assert_eq!(peer.screen().rows(0, 4).nth(1).unwrap(), "i中l");
        assert!(peer.screen().cell(1, 2).unwrap().is_wide_continuation());
        assert!(!peer.screen().alternate_screen());
        assert_eq!(peer.callbacks().0, 1);
        assert_eq!(peer.callbacks().1, b"projected-title");
        let before = terminal.screen().state_formatted();
        let old_view = view;
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(
            terminal
                .write_output_viewport(b"Z", &mut view, false, 4096, &mut Reject)
                .is_err()
        );
        assert_eq!(terminal.screen().state_formatted(), before);
        assert_eq!(view, old_view);
        assert!(
            terminal
                .write_output_viewport(b"Z", &mut view, false, 0, &mut Vec::new())
                .is_err()
        );
        assert_eq!(terminal.screen().state_formatted(), before);
    }
}
