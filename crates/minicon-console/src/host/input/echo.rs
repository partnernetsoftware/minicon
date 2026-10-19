//! Repaint native cooked text using the shared parser's cell layout.
use super::{MAX_LINE, PendingRead, edit::Outcome};
use crate::host::{ConsoleHost, output::echo_fragment};
use std::io::{self, Write};

type Point = (i32, u16);

struct Layout {
    parser: vt100::Parser,
    end: Point,
    extent: Point,
}

impl Layout {
    fn new(line: &[u16], column: u16, columns: u16, mode: u32, flush: bool) -> Self {
        // Two drawing rows plus bounded history retain every rendered line.
        // Width, padding, combining cells and delayed wrap have one owner.
        let mut parser = vt100::Parser::new(2, columns, MAX_LINE * 2 + 4);
        parser.process(if mode & 2 != 0 {
            b"\x1b[?7h"
        } else {
            b"\x1b[?7l"
        });
        if column == columns {
            // A pending wrap follows an existing prompt cell. The scratch
            // prefix is not published, but its occupied edge must participate
            // in the common parser's wrap decision.
            parser.screen_mut().write_cell_text(0, columns - 1, " ");
        }
        assert!(parser.screen_mut().set_cursor_position(0, column));
        let mut decoder = crate::terminal::Utf16Decoder::default();
        for &unit in line {
            for character in decoder.push(unit) {
                Self::character(&mut parser, character, mode, columns);
            }
        }
        if flush && line.last().is_some_and(|u| (0xd800..=0xdbff).contains(u)) {
            Self::character(&mut parser, char::REPLACEMENT_CHARACTER, mode, columns);
        }
        let (row, column) = parser.screen().cursor_position();
        let end = (
            parser.screen().scrollback_depth() as i32 + i32::from(row),
            column,
        );
        let extent = if mode & 2 == 0
            && parser
                .screen()
                .cell(row, columns - 1)
                .is_some_and(|c| c.has_contents() || c.is_wide_continuation())
        {
            (end.0, columns)
        } else {
            end
        };
        Self {
            parser,
            end,
            extent,
        }
    }

    fn character(parser: &mut vt100::Parser, character: char, mode: u32, columns: u16) {
        let column = parser.screen().cursor_position().1;
        let (bytes, eager) = echo_fragment(character, mode, column, columns);
        parser.process(&bytes);
        if eager && parser.screen().cursor_position().1 >= columns {
            parser.process(b"\r\n");
        }
    }

    fn row(&mut self, row: i32) -> u16 {
        let depth = self.parser.screen().scrollback_depth();
        self.parser
            .screen_mut()
            .set_scrollback(depth.saturating_sub(row as usize));
        (row as usize).saturating_sub(depth) as u16
    }

    fn wrapped(&mut self, row: i32) -> bool {
        let row = self.row(row);
        self.parser.screen().row_wrapped(row)
    }

    fn text(&mut self, row: i32, start: u16, end: u16) -> Vec<u8> {
        let row = self.row(row);
        let mut bytes = Vec::new();
        for column in start..end {
            let cell = self.parser.screen().cell(row, column).unwrap();
            if cell.is_wide_continuation() {
                continue;
            }
            if cell.has_contents() {
                bytes.extend_from_slice(cell.contents().as_bytes());
            } else {
                bytes.push(b' ');
            }
        }
        bytes
    }

    fn matches(&mut self, screen: &vt100::Screen, origin: Point) -> bool {
        let (rows, columns) = screen.size();
        let mut visible = false;
        for logical in 0..=self.extent.0 {
            let physical = origin.0 + logical;
            if physical < 0 || physical >= i32::from(rows) {
                continue;
            }
            let start = if logical == 0 { origin.1 } else { 0 };
            let end = if logical == self.extent.0 {
                self.extent.1.min(columns)
            } else {
                columns
            };
            let row = self.row(logical);
            for col in start..end {
                visible = true;
                let a = self.parser.screen().cell(row, col).unwrap();
                let b = screen.cell(physical as u16, col).unwrap();
                let text = |cell: &vt100::Cell| {
                    if cell.has_contents() {
                        cell.contents().to_owned()
                    } else {
                        " ".to_owned()
                    }
                };
                if text(a) != text(b) || a.is_wide_continuation() != b.is_wide_continuation() {
                    return false;
                }
            }
        }
        visible || self.end == (0, origin.1)
    }
}

pub(super) struct Echo {
    origin: Point,
    end: Point,
    columns: u16,
    mode: u32,
    prefix: Vec<u8>,
}

impl Echo {
    fn new(screen: &vt100::Screen, line: &[u16], mode: u32) -> io::Result<Self> {
        let (row, column) = screen.cursor_position();
        let columns = screen.size().1;
        let mut origin = (i32::from(row), column);
        let mut end = (0, column);
        if !line.is_empty() {
            let zero = Layout::new(line, 0, columns, mode, false).end;
            let distance = zero.0 * i32::from(columns) + i32::from(zero.1);
            let guess = (i32::from(column) - distance).rem_euclid(i32::from(columns)) as u16;
            let mut found = false;
            let cost = line.len() + usize::from(columns) * 2;
            let mut work = cost;
            for start in std::iter::once(guess).chain((0..=columns).filter(|start| *start != guess))
            {
                if start == columns && mode & 2 == 0 {
                    continue;
                }
                work += cost;
                if work > 1024 * 1024 {
                    return Err(io::Error::from(io::ErrorKind::QuotaExceeded));
                }
                let mut layout = Layout::new(line, start, columns, mode, false);
                let candidate = (i32::from(row) - layout.end.0, start);
                if layout.end.1 == column && layout.matches(screen, candidate) {
                    origin = candidate;
                    end = layout.extent;
                    found = true;
                    break;
                }
            }
            if !found {
                // nInitialChars is valid even when the caller has not drawn
                // it. Preserve the API text and start publication at the
                // current cursor instead of rejecting the pending read.
                origin = (i32::from(row), column);
                end = (0, column);
            }
        }
        let prefix = if origin.0 >= 0 && origin.1 > 0 && origin.1 < columns {
            screen
                .rows_formatted(0, origin.1)
                .nth(origin.0 as usize)
                .unwrap()
        } else {
            Vec::new()
        };
        if prefix.len() > 1024 * 1024 {
            return Err(io::Error::from(io::ErrorKind::QuotaExceeded));
        }
        Ok(Self {
            origin,
            end,
            columns,
            mode,
            prefix,
        })
    }
}

fn position(parser: &mut vt100::Parser, row: i32, column: u16) -> io::Result<()> {
    if row < 0 || !parser.screen_mut().set_cursor_position(row as u16, column) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid cooked drawing position",
        ));
    }
    Ok(())
}

impl ConsoleHost<'_> {
    pub(super) fn prepare_cooked_echo(&self, read: &mut PendingRead<'_>) -> io::Result<()> {
        if read.echo.is_none() {
            read.echo = Some(Echo::new(
                self.terminal.screen(),
                &read.line,
                self.output_mode,
            )?);
        }
        Ok(())
    }

    pub(super) fn redraw_cooked_echo(
        &mut self,
        read: &mut PendingRead<'_>,
        outcome: &Outcome,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let state = read.echo.as_mut().unwrap();
        let (text, cursor, flush) = match outcome {
            Outcome::Continue => (&read.line[..], read.editor.cursor(), false),
            Outcome::Enter => {
                let n = if self.input_mode & 1 != 0 { 2 } else { 1 };
                (&read.line[..read.line.len() - n], read.line.len() - n, true)
            }
            Outcome::Wake(_) => (
                &read.line[..read.line.len() - 1],
                read.line.len() - 1,
                false,
            ),
        };
        self.paint_cooked_echo(state, text, cursor, flush, output)?;
        if matches!(outcome, Outcome::Enter) {
            self.echo_unit(&mut read.echo_decoder, 13, output)?;
            if self.input_mode & 1 != 0 {
                self.echo_unit(&mut read.echo_decoder, 10, output)?;
            }
        }
        Ok(())
    }

    fn paint_cooked_echo(
        &mut self,
        state: &mut Echo,
        text: &[u16],
        cursor: usize,
        flush: bool,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let (rows, columns) = self.terminal.screen().size();
        if state.columns != columns || state.mode != self.output_mode {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "changed cooked geometry/mode requires relayout",
            ));
        }
        let mut layout = Layout::new(text, state.origin.1, columns, self.output_mode, flush);
        let at = Layout::new(
            &text[..cursor],
            state.origin.1,
            columns,
            self.output_mode,
            flush,
        )
        .end;
        let mut parser = vt100::Parser::new(rows, columns, 0);
        *parser.screen_mut() = self.terminal.screen().clone();
        for logical in 0..=state.end.0 {
            let physical = state.origin.0 + logical;
            if physical < 0 || physical >= i32::from(rows) {
                continue;
            }
            let start = if logical == 0 { state.origin.1 } else { 0 };
            let end = if logical == state.end.0 {
                state.end.1.min(columns)
            } else {
                columns
            };
            if end > start {
                position(&mut parser, physical, start)?;
                parser.process(format!("\x1b[{}X", end - start).as_bytes());
            }
        }
        let physical = state.origin.0 + at.0;
        let shift = if physical < 0 {
            -physical
        } else if physical >= i32::from(rows) {
            i32::from(rows) - 1 - physical
        } else {
            0
        };
        if shift != 0 {
            parser.process(
                format!(
                    "\x1b[{}{}",
                    shift.unsigned_abs(),
                    if shift < 0 { 'S' } else { 'T' }
                )
                .as_bytes(),
            );
        }
        let origin = (state.origin.0 + shift, state.origin.1);
        if shift > 0 && origin.0 >= 0 && !state.prefix.is_empty() {
            position(&mut parser, origin.0, 0)?;
            parser.process(&state.prefix);
        }
        parser.process(&self.terminal.screen().attributes_formatted());
        let mut previous: Option<(i32, bool)> = None;
        for logical in 0..=layout.extent.0 {
            let physical = origin.0 + logical;
            if physical < 0 || physical >= i32::from(rows) {
                continue;
            }
            let start = if logical == 0 { origin.1 } else { 0 };
            let end = if logical == layout.extent.0 {
                layout.extent.1.min(columns)
            } else {
                columns
            };
            if previous.is_none_or(|(row, wrapped)| row + 1 != logical || !wrapped) {
                position(&mut parser, physical, start)?;
            }
            parser.process(&layout.text(logical, start, end));
            previous = Some((logical, layout.wrapped(logical)));
        }
        if !parser.at_boundary() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unfinished cooked projection",
            ));
        }
        let cursor_row = origin.0 + at.0;
        if !parser
            .screen_mut()
            .set_cursor_position(cursor_row as u16, at.1)
        {
            return Err(io::Error::other("invalid cooked cursor layout"));
        }
        let next = parser.screen().clone();
        if let Some(mut view) = self.viewport {
            let proposed = view.following_cursor(&next);
            self.terminal.commit_viewport_screen(
                next,
                &mut view,
                proposed,
                1024 * 1024,
                false,
                output,
            )?;
            self.viewport = Some(view);
        } else {
            self.commit_screen_update(next, output)?
                .map_err(io::Error::from_raw_os_error)?;
        }
        state.origin = origin;
        state.end = layout.extent;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Reject;
    impl Write for Reject {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn rust_console_cooked_echo_commits_after_sink_and_preserves_client_grammar() {
        let mut host = ConsoleHost::new();
        host.set_output_mode(15, &mut Vec::new()).unwrap();
        host.write_ansi(b">abcd\x1b[3", &mut Vec::new()).unwrap();
        let initial: Vec<_> = "abcd".encode_utf16().collect();
        let mut state = Echo::new(host.terminal.screen(), &initial, host.output_mode).unwrap();
        let before = host.terminal.screen().state_formatted();
        let position = (state.origin, state.end);
        let text: Vec<_> = "abXcd".encode_utf16().collect();
        assert_eq!(
            host.paint_cooked_echo(&mut state, &text, 3, false, &mut Reject)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!((state.origin, state.end), position);
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        host.paint_cooked_echo(&mut state, &text, 3, false, &mut Vec::new())
            .unwrap();
        assert_eq!(host.terminal.screen().cursor_position(), (0, 4));
        assert_eq!(state.end, (0, 6));
        host.write_ansi(b"1m!", &mut Vec::new()).unwrap();
        assert_eq!(host.terminal.screen().rows(0, 80).next().unwrap(), ">abX!d");
        assert_eq!(
            host.terminal.screen().cell(0, 4).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
    }

    #[test]
    fn rust_console_cooked_echo_recovers_scrolled_prompt_and_wrapped_rows() {
        let mut host = ConsoleHost::new();
        host.resize(16, 8, &mut Vec::new()).unwrap();
        host.viewport = Some(crate::terminal::TerminalViewport::full(8, 16));
        host.set_output_mode(15, &mut Vec::new()).unwrap();
        host.write_ansi(b"\x1b[31m>\x1b[m", &mut Vec::new())
            .unwrap();
        let mut state = Echo::new(host.terminal.screen(), &[], host.output_mode).unwrap();
        let text: Vec<_> = "a".repeat(150).encode_utf16().collect();
        host.paint_cooked_echo(&mut state, &text, text.len(), false, &mut Vec::new())
            .unwrap();
        assert!(state.origin.0 < 0);
        let before = host.terminal.screen().state_formatted();
        let view = host.viewport;
        host.paint_cooked_echo(&mut state, &text, 0, false, &mut Reject)
            .unwrap_err();
        assert!(state.origin.0 < 0);
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!(host.viewport, view);
        host.paint_cooked_echo(&mut state, &text, 0, false, &mut Vec::new())
            .unwrap();
        assert_eq!(state.origin, (0, 1));
        assert_eq!(host.terminal.screen().cell(0, 0).unwrap().contents(), ">");
        assert_eq!(
            host.terminal.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(host.terminal.screen().cursor_position(), (0, 1));
        assert!(host.terminal.screen().row_wrapped(0));
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&host.terminal.screen().state_formatted());
        assert_eq!(peer.screen().cell(0, 0), host.terminal.screen().cell(0, 0));
        assert_eq!(peer.screen().cursor_position(), (0, 1));
        host.set_output_mode(5, &mut Vec::new()).unwrap();
        host.write_ansi(b"\x1b[2J\x1b[H>", &mut Vec::new()).unwrap();
        let mut state = Echo::new(host.terminal.screen(), &[], host.output_mode).unwrap();
        let text: Vec<_> = "abcdefghijklmnopXqr".encode_utf16().collect();
        host.paint_cooked_echo(&mut state, &text, text.len(), false, &mut Vec::new())
            .unwrap();
        assert_eq!(
            host.terminal.screen().rows(0, 16).next().unwrap(),
            ">abcdefghijklmnr"
        );
        assert_eq!(host.terminal.screen().cursor_position(), (0, 15));
        let mut host = ConsoleHost::new();
        host.resize(1, 8, &mut Vec::new()).unwrap();
        host.viewport = Some(crate::terminal::TerminalViewport::full(8, 1));
        let mut wire = Vec::new();
        host.set_output_mode(15, &mut wire).unwrap();
        host.write_ansi(b">", &mut wire).unwrap();
        let mut state = Echo::new(host.terminal.screen(), &[], host.output_mode).unwrap();
        let text: Vec<_> = "Xc".encode_utf16().collect();
        host.paint_cooked_echo(&mut state, &text, 2, false, &mut wire)
            .unwrap();
        let mut peer = vt100::Parser::new(8, 1, 0);
        peer.process(&wire);
        assert!(host.terminal.screen().row_wrapped(0));
        assert!(peer.screen().row_wrapped(0));
    }

    #[test]
    fn rust_console_cooked_echo_rejects_unmatched_initial_text_with_bounded_search() {
        let host = ConsoleHost::new();
        let screen = host.terminal.screen();
        let before = screen.state_formatted();
        let text = vec![97; MAX_LINE];
        assert_eq!(
            Echo::new(screen, &text, host.output_mode)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::QuotaExceeded
        );
        assert_eq!(screen.state_formatted(), before);
    }
}
