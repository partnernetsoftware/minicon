//! Native percentage metadata with shared, ordered cursor visibility.
use super::{ConsoleHost, INVALID_PARAMETER};
use std::io::{self, Write};

impl ConsoleHost<'_> {
    pub(super) fn set_cursor_info(
        &mut self,
        size: u32,
        visible: bool,
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        if !(1..=100).contains(&size) {
            return Ok(Err(INVALID_PARAMETER));
        }
        // The pinned VT writer projects DECTCEM. A native percentage is not
        // an xterm shape or a promise of pixel-exact frontend geometry.
        self.emit_native(if visible { b"\x1b[?25h" } else { b"\x1b[?25l" }, output)?;
        self.cursor_size = size;
        Ok(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_cursor_info_validates_and_commits_after_output() {
        let mut host = ConsoleHost::new();
        host.set_output_mode(7, &mut Vec::new()).unwrap();
        host.write_ansi(b"\x1b[6 q\x1b[31mX\x1b[3", &mut Vec::new())
            .unwrap();
        let before = host.terminal.screen().clone();
        for size in [0, 101, u32::MAX] {
            let mut output = Vec::new();
            assert_eq!(
                host.set_cursor_info(size, false, &mut output).unwrap(),
                Err(INVALID_PARAMETER)
            );
            assert!(output.is_empty());
        }
        assert_eq!(
            host.set_cursor_info(63, false, &mut Vec::new()).unwrap(),
            Ok(())
        );
        assert_eq!(host.cursor_size, 63);
        assert!(host.terminal.screen().hide_cursor());
        assert_eq!(
            host.terminal.screen().cursor_shape(),
            crate::terminal::CursorShape::Bar
        );
        assert!(!host.terminal.screen().cursor_blinking());
        assert_eq!(
            host.terminal.screen().cursor_position(),
            before.cursor_position()
        );
        assert_eq!(host.terminal.screen().contents(), before.contents());
        assert_eq!(
            host.terminal.screen().attributes_formatted(),
            before.attributes_formatted()
        );
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(host.set_cursor_info(100, true, &mut Reject).is_err());
        assert_eq!(host.cursor_size, 63);
        assert!(host.terminal.screen().hide_cursor());
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        host.write_ansi(b"2mY", &mut Vec::new()).unwrap();
        assert_eq!(
            host.terminal.screen().fgcolor(),
            crate::terminal::Color::Idx(2)
        );
        assert_eq!(host.terminal.screen().contents(), "XY");
    }
}
