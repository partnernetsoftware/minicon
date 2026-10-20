//! Cooked history keys compose the shared native store and the existing editor.
use super::*;

impl ConsoleHost<'_> {
    pub(super) fn edit_history_key(
        &mut self,
        read: &mut PendingRead<'_>,
        key: u16,
        controls: u32,
    ) -> io::Result<bool> {
        let owner = read.request.descriptor.process;
        if key == 0x76 && controls & 3 != 0 {
            self.histories.clear(owner);
            return Ok(true);
        }
        if controls & 0xf != 0 {
            return Ok(false);
        }
        match key {
            0x26 | 0x28 | 0x74 => {
                if let Some(line) = self.histories.recall(owner, key != 0x28) {
                    read.editor.replace(&mut read.line, line);
                }
            }
            0x21 | 0x22 => {
                if let Some(line) = self.histories.boundary(owner, key == 0x21) {
                    read.editor.replace(&mut read.line, line);
                }
            }
            0x27 | 0x70 if read.editor.cursor() == read.line.len() => {
                if let Some(template) = self.histories.template(owner) {
                    read.editor.copy_template(&mut read.line, template, false)?;
                }
            }
            0x72 => {
                if let Some(template) = self.histories.template(owner) {
                    read.editor.copy_template(&mut read.line, template, true)?;
                }
            }
            0x77 => {
                if let Some(line) = self
                    .histories
                    .search(owner, &read.line[..read.editor.cursor()])
                {
                    read.editor.replace_matching(&mut read.line, line);
                }
            }
            27 => {
                self.histories.reset(owner);
                return Ok(false);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
