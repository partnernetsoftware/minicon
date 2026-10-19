//! Bounded native string writes; layout and cell invariants are shared.
use super::{ConsoleHost, INVALID_PARAMETER, NOT_SUPPORTED};
use std::io::{self, Write};
use windows_sys::Win32::Globalization::{MultiByteToWideChar, WideCharToMultiByte};

const MAX_BYTES: usize = 1024 * 1024;

impl ConsoleHost<'_> {
    pub(super) fn write_output_string(
        &mut self,
        kind: u32,
        column: u16,
        row: u16,
        bytes: &[u8],
        output: &mut impl Write,
    ) -> io::Result<Result<usize, i32>> {
        if !(1..=4).contains(&kind) || kind != 1 && !bytes.len().is_multiple_of(2) {
            return Ok(Err(INVALID_PARAMETER));
        }
        if bytes.len() > MAX_BYTES {
            return Ok(Err(NOT_SUPPORTED));
        }
        if bytes.is_empty() {
            return Ok(Ok(0));
        }
        let (rows, columns) = self.terminal.screen().size();
        if row >= rows || column >= columns {
            return Ok(Err(INVALID_PARAMETER));
        }
        let mut next = self.terminal.screen().clone();
        let used = if kind == 3 {
            let attributes: Vec<_> = bytes
                .chunks_exact(2)
                .map(|n| u16::from_le_bytes([n[0], n[1]]))
                .collect();
            if attributes.iter().any(|a| a & !0xc0ff != 0) {
                return Ok(Err(NOT_SUPPORTED));
            }
            next.write_cell_styles(
                row,
                column,
                attributes.into_iter().map(super::output::legacy_style),
            )
        } else {
            let units = if kind == 1 {
                let mut units = vec![0; bytes.len()];
                // SAFETY: disjoint initialized buffers bounded to one MiB;
                // a conversion of n bytes needs at most n UTF-16 units.
                let count = unsafe {
                    MultiByteToWideChar(
                        self.output_code_page,
                        0,
                        bytes.as_ptr(),
                        bytes.len() as i32,
                        units.as_mut_ptr(),
                        units.len() as i32,
                    )
                };
                if count == 0 {
                    return Ok(Err(INVALID_PARAMETER));
                }
                units.truncate(count as usize);
                units
            } else {
                bytes
                    .chunks_exact(2)
                    .map(|n| u16::from_le_bytes([n[0], n[1]]))
                    .collect()
            };
            let Ok(text) = String::from_utf16(&units) else {
                return Ok(Err(NOT_SUPPORTED));
            };
            let consumed = next.write_cell_text(row, column, &text);
            let used = text
                .chars()
                .take(consumed)
                .map(char::len_utf16)
                .sum::<usize>();
            if kind == 1 && used != 0 {
                // SAFETY: initialized prefix of the same bounded conversion;
                // query the code-page byte count without an output allocation.
                let count = unsafe {
                    WideCharToMultiByte(
                        self.output_code_page,
                        0,
                        units.as_ptr(),
                        used as i32,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                    )
                };
                if count == 0 {
                    return Ok(Err(INVALID_PARAMETER));
                }
                count as usize
            } else {
                used
            }
        };
        if let Err(status) = self.commit_screen_update(next, output)? {
            return Ok(Err(status));
        }
        Ok(Ok(used))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_indexed_output_bounds_decoding_and_sink_commit() {
        let mut host = ConsoleHost::new();
        let before = host.terminal.screen().state_formatted();
        for (kind, data, status) in [
            (2, vec![0], INVALID_PARAMETER),
            (2, vec![0, 0xd8], NOT_SUPPORTED),
            (3, vec![0, 4], NOT_SUPPORTED),
            (5, vec![0, 0], INVALID_PARAMETER),
            (2, vec![0; MAX_BYTES + 2], NOT_SUPPORTED),
        ] {
            assert_eq!(
                host.write_output_string(kind, 0, 0, &data, &mut Vec::new())
                    .unwrap(),
                Err(status)
            );
        }
        assert_eq!(host.terminal.screen().state_formatted(), before);
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
            host.write_output_string(2, 0, 0, b"X\0", &mut Reject)
                .is_err()
        );
        assert_eq!(host.terminal.screen().state_formatted(), before);
        let emoji: Vec<_> = "😀X".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(
            host.write_output_string(2, 119, 29, &emoji, &mut Vec::new())
                .unwrap(),
            Ok(0)
        );
        assert_eq!(
            host.write_output_string(2, 118, 29, &emoji, &mut Vec::new())
                .unwrap(),
            Ok(2)
        );
        assert_eq!(
            host.terminal.screen().cell(29, 118).unwrap().contents(),
            "😀"
        );
    }
}
