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
    fn write_output_bytes(&mut self, mut bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        while let Some((&byte, rest)) = bytes.split_first() {
            let length = self
                .terminal
                .text_prefix(bytes, self.output_mode & ENABLE_PROCESSED_OUTPUT != 0);
            if length != 0 {
                self.write_text_batch(&bytes[..length], output)?;
                bytes = &bytes[length..];
                continue;
            }
            bytes = rest;
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

    fn write_text_batch(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 text prefix"))?;
        let vt = self.output_mode & VT_OUTPUT == VT_OUTPUT;
        let wrap = self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0;
        let columns = self.terminal.screen().size().1;
        let mut column = self.terminal.screen().cursor_position().1;
        let mut previous_cr = self.previous_output_cr;
        let mut translated = Vec::with_capacity(bytes.len());
        for character in text.chars() {
            match character {
                '\r' => {
                    translated.push(b'\r');
                    column = 0;
                }
                '\n' => {
                    if self.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0
                        && (!previous_cr || column != 0)
                    {
                        translated.push(b'\r');
                        column = 0;
                    }
                    translated.push(b'\n');
                }
                _ => {
                    let mut encoded = [0; 4];
                    translated.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
                    let width = crate::terminal::Screen::scalar_width(character).unwrap_or(0);
                    if width <= columns {
                        if wrap {
                            if column > columns - width {
                                column = 0;
                            }
                            column += width;
                        } else if column <= columns - width {
                            column = (column + width).min(columns - 1);
                        }
                    }
                    if !vt && wrap && column >= columns {
                        translated.extend_from_slice(b"\r\n");
                        column = 0;
                    }
                }
            }
            previous_cr = character == '\r';
        }
        self.emit_framed(&translated, output)?;
        self.previous_output_cr = previous_cr;
        Ok(())
    }

    fn write_legacy_printable(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        self.emit(bytes, output)?;
        let (_, columns) = self.terminal.screen().size();
        if self.terminal.at_boundary()
            && self.output_mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0
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
        let mut run = Vec::new();
        for unit in units {
            for character in self.output_decoder.push(unit) {
                if !character.is_control()
                    || (self.output_mode & ENABLE_PROCESSED_OUTPUT != 0
                        && matches!(character, '\r' | '\n'))
                {
                    if run.len() + character.len_utf8() > crate::terminal::TEXT_BATCH_BYTES {
                        self.write_output_bytes(&run, output)?;
                        run.clear();
                    }
                    let mut encoded = [0; 4];
                    run.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
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
        Err(error)
            if error.raw_os_error() == Some(1450)
                || crate::terminal::ProjectionLimit::from_error(&error).is_some() =>
        {
            Ok(0xc000_009a_u32 as i32)
        }
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
    fn rust_console_unicode_batches_preserve_modes_margins_and_scalar_model() {
        struct Count(Vec<u8>, usize);
        impl Write for Count {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.1 += 1;
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        fn host(mode: u32) -> ConsoleHost<'static> {
            let mut host = ConsoleHost::new();
            host.resize(80, 32, &mut Vec::new()).unwrap();
            host.set_buffer_size(160, 64, &mut Vec::new())
                .unwrap()
                .unwrap();
            host.set_output_mode(mode, &mut Vec::new()).unwrap();
            host.output_code_page = 65001;
            host
        }
        let text = "A中😀BCe\u{301}".repeat(256);
        for mode in [0, 2, 3, 7, 11, 15] {
            let mut batched = host(mode);
            let mut count = Count(Vec::new(), 0);
            batched
                .write_utf16(text.encode_utf16(), &mut count)
                .unwrap();
            assert!(
                count.1 <= 2,
                "mode {mode}: Unicode published {} individual updates",
                count.1
            );
            let text = "A中😀BCe\u{301}".repeat(32);
            let mut batched = host(mode);
            batched
                .write_utf16(text.encode_utf16(), &mut Vec::new())
                .unwrap();
            let mut scalar = host(mode);
            for character in text.chars() {
                scalar
                    .write_utf16(
                        character.encode_utf16(&mut [0; 2]).iter().copied(),
                        &mut Vec::new(),
                    )
                    .unwrap();
            }
            assert_eq!(
                batched.terminal.screen().state_formatted(),
                scalar.terminal.screen().state_formatted(),
                "mode {mode}"
            );
        }
        let mut long = host(7);
        let mut measured = Count(Vec::new(), 0);
        long.write_utf16(text.repeat(24).encode_utf16(), &mut measured)
            .unwrap();
        assert!(
            measured.1 <= 4,
            "encoded text must use bounded runs, got {} publications",
            measured.1
        );
        for first in [1, 2] {
            let mut host = ConsoleHost::new();
            host.resize(8, 4, &mut Vec::new()).unwrap();
            host.set_buffer_size(8, 8, &mut Vec::new())
                .unwrap()
                .unwrap();
            host.output_code_page = 65001;
            host.set_output_mode(7, &mut Vec::new()).unwrap();
            host.write_ansi(b"12345678", &mut Vec::new()).unwrap();
            let chinese = "中".as_bytes();
            host.write_ansi(&chinese[..first], &mut Vec::new()).unwrap();
            host.set_output_mode(3, &mut Vec::new()).unwrap();
            host.write_ansi(&chinese[first..], &mut Vec::new()).unwrap();
            assert_eq!(
                host.terminal.screen().rows(0, 8).next().unwrap(),
                "12345678"
            );
            assert_eq!(host.terminal.screen().rows(0, 8).nth(1).unwrap(), "中");
        }
        for mode in [0, 2, 3, 7, 11, 15] {
            for columns in [1, 2, 7, 8, 17] {
                let setup = || {
                    let mut host = ConsoleHost::new();
                    host.resize(columns, 4, &mut Vec::new()).unwrap();
                    host.set_buffer_size(columns, 4, &mut Vec::new())
                        .unwrap()
                        .unwrap();
                    host.set_output_mode(mode, &mut Vec::new()).unwrap();
                    host.output_code_page = 65001;
                    host
                };
                let mut batched = setup();
                let mut scalar = setup();
                let text = "\u{301}A中😀e\u{301}B\r\nC\rD\nZ\x07\x1b[31m中Z";
                batched
                    .write_utf16(text.encode_utf16(), &mut Vec::new())
                    .unwrap();
                for c in text.chars() {
                    scalar
                        .write_utf16(c.encode_utf16(&mut [0; 2]).iter().copied(), &mut Vec::new())
                        .unwrap();
                }
                assert_eq!(
                    batched.terminal.screen().state_formatted(),
                    scalar.terminal.screen().state_formatted(),
                    "mode {mode}, columns {columns}"
                );
            }
        }
    }

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
