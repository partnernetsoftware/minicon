//! Win32 inclusive geometry and fill conversion; movement lives in the common library.
use super::{ConsoleHost, INVALID_PARAMETER, NOT_SUPPORTED};
use crate::terminal::CellRect;
use std::io::{self, Write};

fn unit(body: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([body[at], body[at + 1]])
}

fn rectangle(body: &[u8]) -> CellRect {
    CellRect {
        left: i32::from(unit(body, 0) as i16),
        top: i32::from(unit(body, 2) as i16),
        right: i32::from(unit(body, 4) as i16) + 1,
        bottom: i32::from(unit(body, 6) as i16) + 1,
    }
}

impl ConsoleHost<'_> {
    pub(super) fn scroll_output(
        &mut self,
        body: &[u8],
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        if body.len() < 26 {
            return Ok(Err(INVALID_PARAMETER));
        }
        let source = rectangle(body);
        if source.left >= source.right || source.top >= source.bottom {
            return Ok(Err(INVALID_PARAMETER));
        }
        let clip = (body[16] != 0).then(|| rectangle(&body[8..]));
        let attribute = unit(body, 24);
        if attribute & !0xc0ff != 0 {
            return Ok(Err(NOT_SUPPORTED));
        }
        let fill = if body[17] != 0 {
            char::from_u32(u32::from(unit(body, 22))).ok_or(NOT_SUPPORTED)
        } else {
            self.decode_single_byte_cell(body[22])
        };
        let fill = match fill {
            Ok(value) => value,
            Err(status) => return Ok(Err(status)),
        };
        let mut next = self.terminal.screen().clone();
        if next
            .move_region(
                source,
                i32::from(unit(body, 20) as i16),
                i32::from(unit(body, 18) as i16),
                clip,
                fill,
                super::output::legacy_style(attribute),
            )
            .is_err()
        {
            return Ok(Err(NOT_SUPPORTED));
        }
        self.commit_screen_update(next, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_scroll_validates_fill_and_commits_after_publication() {
        let mut host = ConsoleHost::new();
        host.write_ansi(b"ABC", &mut Vec::new()).unwrap();
        let before = host.terminal.screen().state_formatted();
        let mut body = [0; 26];
        body[4] = 2;
        body[17] = 1;
        body[18] = 1;
        body[22..24].copy_from_slice(&0x4e2d_u16.to_le_bytes());
        assert_eq!(
            host.scroll_output(&body, &mut Vec::new()).unwrap(),
            Err(NOT_SUPPORTED)
        );
        assert_eq!(host.terminal.screen().state_formatted(), before);
        body[22..24].copy_from_slice(&u16::from(b'.').to_le_bytes());
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(host.scroll_output(&body, &mut Reject).is_err());
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!(host.scroll_output(&body, &mut Vec::new()).unwrap(), Ok(()));
        assert_eq!(host.terminal.screen().rows(0, 4).next().unwrap(), ".ABC");
    }
}
