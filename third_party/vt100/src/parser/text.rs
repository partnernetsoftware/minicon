//! Read-only visual-text lookahead using the exact owned lexer, never a grid clone.
struct TextProbe {
    safe: bool,
    newlines: bool,
}

impl vte::Perform for TextProbe {
    fn print(&mut self, character: char) {
        self.safe &= !character.is_control();
    }
    fn execute(&mut self, byte: u8) {
        self.safe &= self.newlines && matches!(byte, b'\r' | b'\n');
    }
    fn hook(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {
        self.safe = false;
    }
    fn put(&mut self, _: u8) {
        self.safe = false;
    }
    fn unhook(&mut self) {
        self.safe = false;
    }
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {
        self.safe = false;
    }
    fn csi_dispatch(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {
        self.safe = false;
    }
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {
        self.safe = false;
    }
}

impl<CB: crate::Callbacks> super::Parser<CB> {
    pub(crate) fn text_prefix(&self, bytes: &[u8], limit: usize, newlines: bool) -> usize {
        if !self.at_boundary() || bytes.is_empty() {
            return 0;
        }
        let mut lexer = self.parser.clone();
        let mut probe = TextProbe {
            safe: true,
            newlines,
        };
        let mut complete = 0;
        for (index, &byte) in bytes.iter().take(crate::TEXT_BATCH_BYTES).enumerate() {
            if byte == 27
                || byte < 32 && !(newlines && matches!(byte, b'\r' | b'\n'))
                || byte == 127
                || index + 1 - complete > limit
            {
                break;
            }
            lexer.advance(&mut probe, std::slice::from_ref(&byte));
            if !probe.safe {
                break;
            }
            if lexer.at_boundary() {
                if std::str::from_utf8(&bytes[complete..=index]).is_err() {
                    break;
                }
                complete = index + 1;
            }
        }
        complete
    }
}
