//! Windows console text modes projected onto the shared VT screen.
use super::ConsoleHost;
use std::io::{self, Write};
use windows_sys::Win32::Globalization::{MB_USEGLYPHCHARS, MultiByteToWideChar};
use windows_sys::Win32::System::Console::{
    DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WRAP_AT_EOL_OUTPUT,
};
const VT_OUTPUT: u32 = ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;

impl ConsoleHost<'_> {
    pub(super) fn set_text_attribute(
        &mut self,
        attribute: u16,
        output: &mut impl Write,
    ) -> io::Result<()> {
        let foreground = swap_red_blue((attribute & 15) as u8);
        let background = swap_red_blue(((attribute >> 4) & 15) as u8);
        self.emit(
            format!(
                "\x1b[0;38;5;{foreground};48;5;{background};{};{}m",
                if attribute & 0x4000 != 0 { 7 } else { 27 },
                if attribute & 0x8000 != 0 { 4 } else { 24 },
            )
            .as_bytes(),
            output,
        )
    }

    pub(super) fn text_attribute(&self) -> u16 {
        let screen = self.screen.screen();
        let index = |color, default| match color {
            vt100::Color::Idx(index @ 0..=15) => swap_red_blue(index),
            // Extended palette/RGB quantization remains a separate capability.
            _ => default,
        };
        let foreground = index(screen.fgcolor(), 7) | if screen.bold() { 8 } else { 0 };
        let background = index(screen.bgcolor(), 0);
        u16::from(foreground)
            | (u16::from(background) << 4)
            | if screen.inverse() { 0x4000 } else { 0 }
            | if screen.underline() { 0x8000 } else { 0 }
    }

    /// Configure Win32 output flags and keep the shared terminal's wrapping
    /// state aligned. PTY providers can choose their initial VT mode here.
    pub fn set_output_mode(&mut self, requested: u32, output: &mut impl Write) -> io::Result<()> {
        self.output_mode = requested;
        if self.screen.screen().auto_wrap() != (requested & ENABLE_WRAP_AT_EOL_OUTPUT != 0) {
            self.emit(
                if requested & ENABLE_WRAP_AT_EOL_OUTPUT != 0 {
                    b"\x1b[?7h"
                } else {
                    b"\x1b[?7l"
                },
                output,
            )?;
        }
        Ok(())
    }
    fn write_output_bytes(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        for &byte in bytes {
            let vt = self.output_mode & VT_OUTPUT == VT_OUTPUT;
            if vt {
                if byte == b'\n'
                    && self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                    && (!self.previous_output_cr || self.screen.screen().cursor_position().1 != 0)
                {
                    self.emit(b"\r", output)?;
                }
                self.emit(&[byte], output)?;
            } else if self.output_mode & ENABLE_PROCESSED_OUTPUT == 0 {
                self.write_legacy_printable(
                    if byte < 32 || byte == 127 {
                        b" "
                    } else {
                        std::slice::from_ref(&byte)
                    },
                    output,
                )?;
            } else {
                match byte {
                    0 => self.write_legacy_printable(b" ", output)?,
                    7 => self.emit(b"\x07", output)?,
                    8 => {
                        let (row, column) = self.screen.screen().cursor_position();
                        let previous = column.saturating_sub(1);
                        let wide = self
                            .screen
                            .screen()
                            .cell(row, previous)
                            .is_some_and(vt100::Cell::is_wide_continuation);
                        self.emit(
                            format!(
                                "\x1b[{};{}H",
                                row + 1,
                                column.saturating_sub(if wide { 2 } else { 1 }) + 1
                            )
                            .as_bytes(),
                            output,
                        )?;
                    }
                    9 => {
                        let (_, columns) = self.screen.screen().size();
                        let column = self.screen.screen().cursor_position().1.min(columns - 1);
                        for _ in 0..(8 - column % 8).min(columns - column) {
                            self.write_legacy_printable(b" ", output)?;
                        }
                    }
                    10 => {
                        if self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                            && (!self.previous_output_cr
                                || self.screen.screen().cursor_position().1 != 0)
                        {
                            self.emit(b"\r", output)?;
                        }
                        self.emit(b"\n", output)?;
                    }
                    13 => self.emit(b"\r", output)?,
                    1..=31 | 127 => {
                        let mut glyph = 0;
                        // SAFETY: one input byte and one output UTF-16 unit;
                        // this is the Windows OEM control-glyph conversion.
                        if unsafe {
                            MultiByteToWideChar(
                                self.output_code_page,
                                MB_USEGLYPHCHARS,
                                &byte,
                                1,
                                &mut glyph,
                                1,
                            )
                        } == 1
                        {
                            let character = char::from_u32(u32::from(glyph))
                                .unwrap_or(char::REPLACEMENT_CHARACTER);
                            let mut encoded = [0; 4];
                            let printable = if character.is_control() {
                                ' '
                            } else {
                                character
                            };
                            self.write_legacy_printable(
                                printable.encode_utf8(&mut encoded).as_bytes(),
                                output,
                            )?;
                        }
                    }
                    _ => self.write_legacy_printable(&[byte], output)?,
                }
            }
            self.previous_output_cr = byte == b'\r';
        }
        Ok(())
    }

    fn write_legacy_printable(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        self.emit(bytes, output)?;
        let (_, columns) = self.screen.screen().size();
        if self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0
            && self.screen.screen().cursor_position().1 >= columns
        {
            // Legacy console writes wrap immediately, including mode 0x0b;
            // only the VT path retains a delayed right-margin cursor.
            self.emit(b"\r\n", output)?;
        }
        Ok(())
    }

    pub(super) fn write_ansi(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        if self.output_code_page == 65001 {
            return self.write_output_bytes(bytes, output);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let mut wide = vec![0_u16; bytes.len()];
        // SAFETY: input/output point to live, disjoint buffers and lengths fit
        // i32 because read_input bounds every request to at most one MiB.
        let length = unsafe {
            MultiByteToWideChar(
                self.output_code_page,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                wide.as_mut_ptr(),
                wide.len() as i32,
            )
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        self.write_utf16(wide[..length as usize].iter().copied(), output)
    }

    pub(super) fn write_utf16(
        &mut self,
        units: impl IntoIterator<Item = u16>,
        output: &mut impl Write,
    ) -> io::Result<()> {
        for unit in units {
            if let Some(high) = self.pending_high_surrogate.take() {
                if (0xdc00..=0xdfff).contains(&unit) {
                    let value =
                        0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00;
                    self.emit_char(char::from_u32(value).unwrap(), output)?;
                    continue;
                }
                self.emit_char(char::REPLACEMENT_CHARACTER, output)?;
            }
            if (0xd800..=0xdbff).contains(&unit) {
                self.pending_high_surrogate = Some(unit);
            } else {
                self.emit_char(
                    char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER),
                    output,
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn emit(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        output.write_all(bytes)?;
        self.screen.process(bytes);
        self.output_mode = (self.output_mode & !ENABLE_WRAP_AT_EOL_OUTPUT)
            | if self.screen.screen().auto_wrap() {
                ENABLE_WRAP_AT_EOL_OUTPUT
            } else {
                0
            };
        Ok(())
    }

    fn emit_char(&mut self, character: char, output: &mut impl Write) -> io::Result<()> {
        let mut buffer = [0; 4];
        self.write_output_bytes(character.encode_utf8(&mut buffer).as_bytes(), output)
    }
}

fn swap_red_blue(index: u8) -> u8 {
    (index & 10) | ((index & 1) << 2) | ((index & 4) >> 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_host_output_utf8_fragments_preserve_scalar_and_crlf_stream() {
        for mode in [3, 7, 15] {
            let mut host = ConsoleHost::new();
            host.output_code_page = 65001;
            let mut output = Vec::new();
            host.set_output_mode(mode, &mut output).unwrap();
            host.write_ansi(&[b'X', 0xe4], &mut output).unwrap();
            assert_eq!(host.screen.screen().cursor_position(), (0, 1));
            host.write_ansi(&[0xb8, 0xad, b'\r'], &mut output).unwrap();
            host.write_ansi(&[b'\n', 0xf0, 0x9f], &mut output).unwrap();
            assert_eq!(host.screen.screen().cursor_position(), (1, 0));
            host.write_ansi(&[0x98, 0x80], &mut output).unwrap();
            assert_eq!(output, "X中\r\n😀".as_bytes());
            assert_eq!(host.screen.screen().cursor_position(), (1, 2));
        }
    }
}
