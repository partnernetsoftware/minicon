//! Native title metadata and its safe, independently framed display form.
use super::ConsoleHost;
use std::io::{self, Write};

impl ConsoleHost<'_> {
    pub(super) fn set_native_title(
        &mut self,
        title: Vec<u16>,
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
            .write_window_title(&display, 1024 * 1024, output)?;
        self.title = title;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
