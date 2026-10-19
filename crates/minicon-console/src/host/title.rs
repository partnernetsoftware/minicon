//! Native title metadata and its safe, independently framed display form.
use super::ConsoleHost;
use std::io::{self, Write};

impl ConsoleHost<'_> {
    pub(super) fn decode_title_payload(
        &self,
        unicode: bool,
        bytes: &[u8],
    ) -> Result<Vec<u16>, i32> {
        if bytes.len() > 65534 || (unicode && !bytes.len().is_multiple_of(2)) {
            return Err(super::INVALID_PARAMETER);
        }
        let units: Vec<u16> = if unicode {
            bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect()
        } else {
            super::encoding::decode(self.input_code_page, bytes)
                .map_err(|_| super::INVALID_PARAMETER)?
        };
        if units.len() > 32767 {
            return Err(super::INVALID_PARAMETER);
        }
        Ok(units)
    }

    pub(super) fn title_reply(
        &self,
        unicode: bool,
        original: bool,
        capacity: usize,
    ) -> Result<(usize, Vec<u8>), i32> {
        let title = if original {
            &self.original_title
        } else {
            &self.title
        };
        if unicode {
            let capacity = capacity / 2;
            // The native reply count excludes a trailing NUL when there is
            // spare room. KernelBase terminates current-title W reads itself;
            // original-title reads expose the returned bytes unchanged.
            let bytes = title
                .iter()
                .take(capacity)
                .enumerate()
                .flat_map(|(i, unit)| if i + 1 == capacity { 0 } else { *unit }.to_le_bytes())
                .collect();
            return Ok((title.len(), bytes));
        }
        let mut bytes = super::encoding::encode(self.input_code_page, title, usize::MAX)
            .map_err(|_| super::INVALID_PARAMETER)?
            .ok_or(super::INVALID_PARAMETER)?;
        if bytes.is_empty() {
            return Ok((0, bytes));
        }
        if capacity < bytes.len() {
            return Ok((0, if capacity == 0 { Vec::new() } else { vec![0] }));
        }
        let needed = bytes.len();
        if capacity > needed {
            bytes.push(0);
        }
        Ok((needed, bytes))
    }

    pub(super) fn set_native_title(
        &mut self,
        title: Vec<u16>,
        output: &mut impl Write,
    ) -> io::Result<()> {
        self.publish_native_title(&title, output)?;
        self.title = title;
        Ok(())
    }

    pub(super) fn publish_native_title(
        &self,
        title: &[u16],
        output: &mut impl Write,
    ) -> io::Result<()> {
        // Windows' VT writer displays C0/DEL using DOS glyphs and C1 as '?'.
        // Keep the original UTF-16 metadata separate from this display form.
        const C0: [char; 32] = [
            ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼', '►',
            '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
        ];
        let display: String = char::decode_utf16(title.iter().copied())
            .map(|ch| match ch.unwrap_or(char::REPLACEMENT_CHARACTER) {
                ch @ '\0'..='\u{1f}' => C0[ch as usize],
                '\u{7f}' => '⌂',
                '\u{80}'..='\u{9f}' => '?',
                ch => ch,
            })
            .collect();
        self.terminal
            .write_window_title(&display, 1024 * 1024, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_title_payloads_bound_native_transfers_and_reply_allocations() {
        let mut host = ConsoleHost::new();
        host.input_code_page = 936;
        host.output_code_page = 65001;
        assert_eq!(
            host.decode_title_payload(false, b"A\xd6\xd0").unwrap(),
            "A中".encode_utf16().collect::<Vec<_>>()
        );
        assert!(host.decode_title_payload(true, &[0]).is_err());
        assert_eq!(
            host.decode_title_payload(true, &vec![0; 65534])
                .unwrap()
                .len(),
            32767
        );
        assert!(host.decode_title_payload(true, &vec![0; 65536]).is_err());
        assert!(
            host.decode_title_payload(false, &vec![b'A'; 32768])
                .is_err()
        );
        assert!(
            host.decode_title_payload(false, &vec![b'A'; 65535])
                .is_err()
        );
        host.input_code_page = 65001;
        assert!(
            host.decode_title_payload(false, "中".repeat(21845).as_bytes())
                .is_err()
        );
        host.input_code_page = 936;
        assert_eq!(
            host.decode_title_payload(true, &[0, 0xd8]).unwrap(),
            vec![0xd800]
        );
        assert!(host.decode_title_payload(false, b"").unwrap().is_empty());
        host.title = "A中".encode_utf16().collect();
        host.original_title = "original".encode_utf16().collect();
        assert_eq!(
            host.title_reply(false, false, usize::MAX).unwrap(),
            (3, vec![b'A', 0xd6, 0xd0, 0])
        );
        assert_eq!(host.title_reply(false, false, 2).unwrap(), (0, vec![0]));
        assert_eq!(host.title_reply(true, false, 3).unwrap(), (2, vec![0, 0]));
        assert_eq!(
            host.title_reply(false, true, usize::MAX).unwrap(),
            (8, b"original\0".to_vec())
        );
    }

    #[test]
    fn rust_console_native_title_commits_after_sink_and_retains_raw_metadata() {
        let mut host = ConsoleHost::new();
        host.title = "old".encode_utf16().collect();
        host.original_title = "original".encode_utf16().collect();
        host.terminal
            .write_output(b"A\x1b[3", &mut io::sink())
            .unwrap();
        let before = host.terminal.screen().state_formatted();
        let title = vec![65, 7, 27, 0x7f, 0x9c, 0xd800, 0xd83d, 0xde00];
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("reject native title"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(host.set_native_title(title.clone(), &mut Reject).is_err());
        assert_eq!(host.title, "old".encode_utf16().collect::<Vec<_>>());
        let mut wire = Vec::new();
        host.set_native_title(title.clone(), &mut wire).unwrap();
        assert_eq!(wire, "\x1b]0;A•←⌂?�😀\x1b\\".as_bytes());
        assert_eq!(host.title, title);
        assert_eq!(
            host.original_title,
            "original".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        assert_eq!(host.terminal.screen().state_formatted(), before);
    }
}
