//! Native inclusive rectangle geometry and bounded CHAR_INFO transport.
use super::{ConsoleHost, INVALID_PARAMETER, NOT_SUPPORTED};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;
use windows_sys::Win32::Globalization::{
    CPINFO, GetCPInfo, MultiByteToWideChar, WideCharToMultiByte,
};

const MAX_BYTES: usize = 1024 * 1024;

struct Rectangle {
    original: [i16; 4],
    width: usize,
    bytes: usize,
}

impl Rectangle {
    fn new(original: [i16; 4]) -> Result<Self, i32> {
        let width = i32::from(original[2]) - i32::from(original[0]) + 1;
        let height = i32::from(original[3]) - i32::from(original[1]) + 1;
        if width <= 0 || height <= 0 {
            return Err(INVALID_PARAMETER);
        }
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(NOT_SUPPORTED)?;
        Ok(Self {
            original,
            width: width as usize,
            bytes,
        })
    }

    fn empty(&self) -> Result<[i16; 4], i32> {
        let [left, top, _, _] = self.original;
        Ok([
            left,
            top,
            left.checked_sub(1).ok_or(INVALID_PARAMETER)?,
            top.checked_sub(1).ok_or(INVALID_PARAMETER)?,
        ])
    }

    fn clipped(&self, size: (u16, u16)) -> Option<[i16; 4]> {
        let [left, top, right, bottom] = self.original.map(i32::from);
        let result = [
            left.max(0),
            top.max(0),
            right.min(i32::from(size.1) - 1),
            bottom.min(i32::from(size.0) - 1),
        ];
        (result[0] <= result[2] && result[1] <= result[3]).then(|| result.map(|n| n as i16))
    }

    fn offset(&self, row: i16, column: i16) -> usize {
        ((i32::from(row) - i32::from(self.original[1])) as usize * self.width
            + (i32::from(column) - i32::from(self.original[0])) as usize)
            * 4
    }
}

impl ConsoleHost<'_> {
    pub(super) fn write_output_rect(
        &mut self,
        region: [i16; 4],
        unicode: bool,
        bytes: &[u8],
        output: &mut impl Write,
    ) -> io::Result<Result<[i16; 4], i32>> {
        let request = match Rectangle::new(region) {
            Ok(value) => value,
            Err(status) => return Ok(Err(status)),
        };
        if bytes.len() < request.bytes || bytes.len() > MAX_BYTES {
            return Ok(Err(INVALID_PARAMETER));
        }
        let Some(clipped) = request.clipped(self.terminal.screen().size()) else {
            return Ok(request.empty());
        };
        let mut next = self.terminal.screen().clone();
        for row in clipped[1]..=clipped[3] {
            let mut column = clipped[0];
            while column <= clipped[2] {
                let at = request.offset(row, column);
                let unit = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                let attribute = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]);
                if attribute & !0xc3ff != 0 {
                    return Ok(Err(NOT_SUPPORTED));
                }
                let character = if unicode {
                    char::from_u32(u32::from(unit))
                } else {
                    match self.rect_ansi_to_wide(unit as u8) {
                        Ok(value) => Some(value),
                        Err(status) => return Ok(Err(status)),
                    }
                };
                let Some(character) = character else {
                    return Ok(Err(NOT_SUPPORTED));
                };
                let width = character.width().unwrap_or(1).max(1);
                match attribute & 0x300 {
                    0 if width == 1 => {}
                    0x100 if width == 2 && column < clipped[2] => {
                        let tail = at + 4;
                        let tail_unit = u16::from_le_bytes([bytes[tail], bytes[tail + 1]]);
                        let tail_attribute = u16::from_le_bytes([bytes[tail + 2], bytes[tail + 3]]);
                        if !unicode
                            || tail_unit != unit
                            || tail_attribute != ((attribute & !0x300) | 0x200)
                        {
                            return Ok(Err(NOT_SUPPORTED));
                        }
                    }
                    _ => return Ok(Err(NOT_SUPPORTED)),
                }
                if !next.write_cell(
                    row as u16,
                    column as u16,
                    character,
                    super::output::legacy_style(attribute & !0x300),
                ) {
                    return Ok(Err(INVALID_PARAMETER));
                }
                column += width as i16;
            }
        }
        let projection = next.contents_diff(self.terminal.screen());
        if projection.len() > MAX_BYTES {
            return Ok(Err(NOT_SUPPORTED));
        }
        output.write_all(&projection)?;
        *self.terminal.screen_mut() = next;
        Ok(Ok(clipped))
    }

    pub(super) fn read_output_rect(
        &self,
        region: [i16; 4],
        unicode: bool,
        capacity: usize,
    ) -> Result<([i16; 4], Vec<u8>), i32> {
        let request = Rectangle::new(region)?;
        if capacity < request.bytes || capacity > MAX_BYTES || !capacity.is_multiple_of(4) {
            return Err(INVALID_PARAMETER);
        }
        let mut bytes = vec![0; capacity];
        let screen = self.terminal.screen();
        let Some(clipped) = request.clipped(screen.size()) else {
            // No readable cells: complete deterministically as the installed
            // SDK reference does, rather than passing a zero-area successful
            // capture into KernelBase's block-copy path.
            return Err(INVALID_PARAMETER);
        };
        for row in clipped[1]..=clipped[3] {
            for column in clipped[0]..=clipped[2] {
                let cell = screen.cell(row as u16, column as u16).unwrap();
                let text = if cell.is_wide_continuation() {
                    screen
                        .cell(row as u16, column as u16 - 1)
                        .unwrap()
                        .contents()
                } else {
                    cell.contents()
                };
                let mut units = text.encode_utf16();
                let unit = units.next().unwrap_or(u16::from(b' '));
                let mut unit = if units.next().is_some() { 0xfffd } else { unit };
                if !unicode {
                    unit = u16::from(self.rect_wide_to_ansi(unit)?);
                }
                let attribute = super::output::legacy_attribute(
                    cell.fgcolor(),
                    cell.bgcolor(),
                    cell.bold(),
                    cell.inverse(),
                    cell.underline(),
                ) | if cell.is_wide() {
                    0x100
                } else if cell.is_wide_continuation() {
                    0x200
                } else {
                    0
                };
                let at = request.offset(row, column);
                bytes[at..at + 2].copy_from_slice(&unit.to_le_bytes());
                bytes[at + 2..at + 4].copy_from_slice(&attribute.to_le_bytes());
            }
        }
        Ok((clipped, bytes))
    }

    fn rect_ansi_to_wide(&self, byte: u8) -> Result<char, i32> {
        if byte.is_ascii() {
            return Ok(char::from(byte));
        }
        let mut info = CPINFO::default();
        let mut unit = 0;
        // SAFETY: numeric CP query and one initialized input/output unit.
        if unsafe { GetCPInfo(self.output_code_page, &mut info) } == 0
            || info.MaxCharSize != 1
            || unsafe { MultiByteToWideChar(self.output_code_page, 0, &byte, 1, &mut unit, 1) } != 1
        {
            return Err(NOT_SUPPORTED);
        }
        char::from_u32(u32::from(unit)).ok_or(NOT_SUPPORTED)
    }

    fn rect_wide_to_ansi(&self, unit: u16) -> Result<u8, i32> {
        let mut byte = 0;
        // SAFETY: one initialized UTF-16 unit, one-byte target and no fallback pointers.
        if unsafe {
            WideCharToMultiByte(
                self.output_code_page,
                0,
                &unit,
                1,
                &mut byte,
                1,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        } != 1
        {
            return Err(NOT_SUPPORTED);
        }
        Ok(byte)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConsoleHost, INVALID_PARAMETER, MAX_BYTES, NOT_SUPPORTED, Rectangle};

    #[test]
    fn rust_console_rectangles_bound_capture_and_commit_only_after_output() {
        assert!(matches!(
            Rectangle::new([0, 0, 32767, 32767]),
            Err(NOT_SUPPORTED)
        ));
        assert!(matches!(
            Rectangle::new([2, 0, 1, 0]),
            Err(INVALID_PARAMETER)
        ));
        let mut host = ConsoleHost::new();
        assert_eq!(
            host.read_output_rect([200, 200, 201, 201], true, 16),
            Err(INVALID_PARAMETER)
        );
        let before = host.terminal.screen().state_formatted();
        assert_eq!(
            host.read_output_rect([0, 0, 0, 0], true, MAX_BYTES + 4),
            Err(INVALID_PARAMETER)
        );
        assert_eq!(
            host.read_output_rect([0, 0, 1, 0], true, 4),
            Err(INVALID_PARAMETER)
        );
        let invalid = [b'A', 0, 7, 0, 0, 0xd8, 7, 0];
        assert_eq!(
            host.write_output_rect([0, 0, 1, 0], true, &invalid, &mut Vec::new())
                .unwrap(),
            Err(NOT_SUPPORTED)
        );
        assert_eq!(host.terminal.screen().state_formatted(), before);
        struct Reject;
        impl std::io::Write for Reject {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reject rectangle before publication"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let one = [b'X', 0, 0x1e, 0];
        assert!(
            host.write_output_rect([0, 0, 0, 0], true, &one, &mut Reject)
                .is_err()
        );
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!(
            host.write_output_rect([0, 0, 0, 0], true, &one, &mut Vec::new())
                .unwrap(),
            Ok([0, 0, 0, 0])
        );
        assert_eq!(
            host.read_output_rect([0, 0, 0, 0], true, 4).unwrap(),
            ([0, 0, 0, 0], one.to_vec())
        );
    }
}
