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
        self.emit_native(
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
        let screen = self.terminal.screen();
        legacy_attribute(
            screen.fgcolor(),
            screen.bgcolor(),
            screen.bold(),
            screen.inverse(),
            screen.underline(),
        )
    }

    /// Configure Win32 output flags and keep the shared terminal's wrapping
    /// state aligned. PTY providers can choose their initial VT mode here.
    pub fn set_output_mode(&mut self, requested: u32, output: &mut impl Write) -> io::Result<()> {
        self.output_mode = requested;
        if self.terminal.screen().auto_wrap() != (requested & ENABLE_WRAP_AT_EOL_OUTPUT != 0) {
            self.emit_native(
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
        // A viewport transaction clones the drawing state once per published
        // batch. Keep ordinary ASCII runs together, including deterministic
        // CR/LF translation, instead of cloning a large buffer per glyph.
        if self.viewport.is_some()
            && self.terminal.at_boundary()
            && (self.terminal.screen().cursor_position().1 < self.terminal.screen().size().1
                || self.output_mode & VT_OUTPUT == VT_OUTPUT)
            && bytes.iter().all(|b| {
                (32..127).contains(b)
                    || self.output_mode & ENABLE_PROCESSED_OUTPUT != 0
                        && matches!(*b, b'\r' | b'\n')
            })
        {
            let vt = self.output_mode & VT_OUTPUT == VT_OUTPUT;
            let wrap = self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0;
            let columns = self.terminal.screen().size().1;
            let mut column = self.terminal.screen().cursor_position().1;
            let mut previous_cr = self.previous_output_cr;
            let mut translated = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                match byte {
                    b'\r' => {
                        translated.push(byte);
                        column = 0;
                    }
                    b'\n' => {
                        if self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                            && (!previous_cr || column != 0)
                        {
                            translated.push(b'\r');
                            column = 0;
                        }
                        translated.push(byte);
                    }
                    _ => {
                        translated.push(byte);
                        column = column.saturating_add(1);
                        if !vt && wrap && column >= columns {
                            translated.extend_from_slice(b"\r\n");
                            column = 0;
                        } else if !wrap {
                            column = column.min(columns - 1);
                        }
                    }
                }
                previous_cr = byte == b'\r';
            }
            self.emit_framed(&translated, output)?;
            self.previous_output_cr = previous_cr;
            return Ok(());
        }

        for &byte in bytes {
            let vt = self.output_mode & VT_OUTPUT == VT_OUTPUT;
            if vt {
                if byte == b'\n'
                    && self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                    && (!self.previous_output_cr || self.terminal.screen().cursor_position().1 != 0)
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
                        let (row, column) = self.terminal.screen().cursor_position();
                        let previous = column.saturating_sub(1);
                        let wide = self
                            .terminal
                            .screen()
                            .cell(row, previous)
                            .is_some_and(vt100::Cell::is_wide_continuation);
                        self.emit_native(
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
                        let (_, columns) = self.terminal.screen().size();
                        let column = self.terminal.screen().cursor_position().1.min(columns - 1);
                        for _ in 0..(8 - column % 8).min(columns - column) {
                            self.write_legacy_printable(b" ", output)?;
                        }
                    }
                    10 => {
                        if self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                            && (!self.previous_output_cr
                                || self.terminal.screen().cursor_position().1 != 0)
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
        let (_, columns) = self.terminal.screen().size();
        if self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0
            && self.terminal.screen().cursor_position().1 >= columns
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
        if self.viewport.is_some() {
            let mut run = Vec::new();
            for unit in units {
                for character in self.output_decoder.push(unit) {
                    if character.is_ascii()
                        && ((32..127).contains(&(character as u8))
                            || self.output_mode & ENABLE_PROCESSED_OUTPUT != 0
                                && matches!(character, '\r' | '\n'))
                    {
                        run.push(character as u8);
                    } else {
                        if !run.is_empty() {
                            self.write_output_bytes(&run, output)?;
                            run.clear();
                        }
                        self.emit_char(character, output)?;
                    }
                }
            }
            if !run.is_empty() {
                self.write_output_bytes(&run, output)?;
            }
        } else {
            for unit in units {
                for character in self.output_decoder.push(unit) {
                    self.emit_char(character, output)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn emit(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        self.emit_framed(bytes, output)
    }

    pub(super) fn echo_unit(
        &mut self,
        decoder: &mut crate::terminal::Utf16Decoder,
        unit: u16,
        output: &mut impl Write,
    ) -> io::Result<()> {
        for character in decoder.push(unit) {
            if self.output_mode & ENABLE_PROCESSED_OUTPUT == 0 && character.is_ascii_control() {
                self.echo_printable(' ', output)?;
            } else {
                match character {
                    '\r' => self.emit_native(b"\r", output)?,
                    '\n' => {
                        if self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                            && self.terminal.screen().cursor_position().1 != 0
                        {
                            self.emit_native(b"\r", output)?;
                        }
                        self.emit_native(b"\n", output)?;
                    }
                    '\t' => {
                        let (_, column) = self.terminal.screen().cursor_position();
                        let columns = self.terminal.screen().size().1;
                        for _ in 0..(8 - column % 8).min(columns.saturating_sub(column)) {
                            self.echo_printable(' ', output)?;
                        }
                    }
                    control if control.is_ascii_control() => {
                        // Cooked input displays control notation instead of
                        // interpreting keyboard bytes as client VT commands.
                        self.echo_printable('^', output)?;
                        self.echo_printable(char::from((control as u8) ^ 64), output)?;
                    }
                    printable => self.echo_printable(printable, output)?,
                }
            }
        }
        Ok(())
    }

    fn echo_printable(&mut self, character: char, output: &mut impl Write) -> io::Result<()> {
        let mut encoded = [0; 4];
        self.emit_native(character.encode_utf8(&mut encoded).as_bytes(), output)?;
        if self.output_mode & VT_OUTPUT != VT_OUTPUT
            && self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0
            && self.terminal.screen().cursor_position().1 >= self.terminal.screen().size().1
        {
            self.emit_native(b"\r\n", output)?;
        }
        Ok(())
    }

    pub(super) fn sync_output_wrap(&mut self) {
        self.output_mode = (self.output_mode & !ENABLE_WRAP_AT_EOL_OUTPUT)
            | if self.terminal.screen().auto_wrap() {
                ENABLE_WRAP_AT_EOL_OUTPUT
            } else {
                0
            };
    }

    fn emit_char(&mut self, character: char, output: &mut impl Write) -> io::Result<()> {
        let mut buffer = [0; 4];
        self.write_output_bytes(character.encode_utf8(&mut buffer).as_bytes(), output)
    }
}

pub(super) fn write_status(result: io::Result<()>) -> io::Result<i32> {
    match result {
        Ok(()) => Ok(super::SUCCESS),
        Err(error) if error.raw_os_error() == Some(1450) => Ok(0xc000_009a_u32 as i32),
        Err(error) => Err(error),
    }
}

fn swap_red_blue(index: u8) -> u8 {
    (index & 10) | ((index & 1) << 2) | ((index & 4) >> 2)
}

pub(super) fn legacy_style(attribute: u16) -> vt100::CellStyle {
    vt100::CellStyle {
        foreground: vt100::Color::Idx(swap_red_blue((attribute & 15) as u8)),
        background: vt100::Color::Idx(swap_red_blue(((attribute >> 4) & 15) as u8)),
        inverse: attribute & 0x4000 != 0,
        underline: attribute & 0x8000 != 0,
        ..vt100::CellStyle::default()
    }
}

pub(super) fn legacy_attribute(
    foreground: vt100::Color,
    background: vt100::Color,
    bold: bool,
    inverse: bool,
    underline: bool,
) -> u16 {
    let index = |color, default| match color {
        vt100::Color::Idx(index @ 0..=15) => swap_red_blue(index),
        // Extended palette/RGB quantization remains a separate capability.
        _ => default,
    };
    u16::from(index(foreground, 7) | if bold { 8 } else { 0 })
        | (u16::from(index(background, 0)) << 4)
        | if inverse { 0x4000 } else { 0 }
        | if underline { 0x8000 } else { 0 }
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
            assert_eq!(host.terminal.screen().cursor_position(), (0, 1));
            host.write_ansi(&[0xb8, 0xad, b'\r'], &mut output).unwrap();
            host.write_ansi(&[b'\n', 0xf0, 0x9f], &mut output).unwrap();
            assert_eq!(host.terminal.screen().cursor_position(), (1, 0));
            host.write_ansi(&[0x98, 0x80], &mut output).unwrap();
            assert_eq!(output, "X中\r\n😀".as_bytes());
            assert_eq!(host.terminal.screen().cursor_position(), (1, 2));
        }
    }
}
