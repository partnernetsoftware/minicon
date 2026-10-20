//! Native cooked-read editing over original UTF-16, independent of echo layout.
use super::MAX_LINE;
use std::io;
use unicode_segmentation::UnicodeSegmentation;

pub(super) struct Editor {
    cursor: usize,
    insert: bool,
}

pub(super) enum Outcome {
    Continue,
    Enter,
    Wake(u32),
}

impl Editor {
    pub(super) fn cursor(&self) -> usize {
        self.cursor
    }
    pub(super) fn new(length: usize, insert: bool) -> Self {
        Self {
            cursor: length,
            insert,
        }
    }

    pub(super) fn handle(
        &mut self,
        line: &mut Vec<u16>,
        character: u16,
        virtual_key: u16,
        controls: u32,
        processed: bool,
        wakeup: u32,
    ) -> io::Result<Outcome> {
        let ctrl = controls & 0xc != 0;
        let plain = controls & 0xf == 0;
        // Editing keys are classified before their character (including ESC),
        // as GetChar/IsCommandLineEditingKey does in the native host.
        match virtual_key {
            0x1b if plain => {
                line.clear();
                self.cursor = 0;
            }
            0x24 if plain || ctrl => {
                if ctrl {
                    line.drain(..self.cursor);
                }
                self.cursor = 0;
            }
            0x23 if plain || ctrl => {
                if ctrl {
                    line.truncate(self.cursor);
                }
                self.cursor = line.len();
            }
            // The installed native console uses ASCII spaces as word separators;
            // punctuation and tabs remain in the word. Scan original units so
            // invalid UTF-16 survives, with no allocations or line growth.
            0x25 if ctrl => {
                let prefix = &line[..self.cursor];
                let end = prefix
                    .iter()
                    .rposition(|&unit| unit != 32)
                    .map_or(0, |i| i + 1);
                self.cursor = prefix[..end]
                    .iter()
                    .rposition(|&unit| unit == 32)
                    .map_or(0, |i| i + 1);
            }
            0x27 if ctrl => {
                let suffix = &line[self.cursor..];
                let end = suffix
                    .iter()
                    .position(|&unit| unit == 32)
                    .unwrap_or(suffix.len());
                self.cursor += end
                    + suffix[end..]
                        .iter()
                        .position(|&unit| unit != 32)
                        .unwrap_or(suffix.len() - end);
            }
            0x25 if plain => self.cursor = previous(line, self.cursor),
            0x27 | 0x70 if plain => self.cursor = next(line, self.cursor),
            0x2d if plain => self.insert = !self.insert,
            0x2e if plain => {
                line.drain(self.cursor..next(line, self.cursor));
            }
            _ => return self.character(line, character, controls, processed, wakeup),
        }
        Ok(Outcome::Continue)
    }

    fn character(
        &mut self,
        line: &mut Vec<u16>,
        character: u16,
        controls: u32,
        processed: bool,
        wakeup: u32,
    ) -> io::Result<Outcome> {
        if character == 0 {
            return Ok(Outcome::Continue);
        }
        if character < 32 && wakeup & (1_u32 << character) != 0 {
            // Completion hands only the edited prefix and wakeup unit back to
            // the caller. The undisplayed suffix is no longer editable.
            line.truncate(self.cursor);
            self.put(line, character)?;
            return Ok(Outcome::Wake(controls));
        }
        if character == 13 {
            line.push(13);
            if processed {
                line.push(10);
            }
            self.cursor = line.len();
            return Ok(Outcome::Enter);
        }
        if character == 8 && processed {
            let start = previous(line, self.cursor);
            line.drain(start..self.cursor);
            self.cursor = start;
        } else {
            self.put(line, character)?;
        }
        Ok(Outcome::Continue)
    }

    fn put(&mut self, line: &mut Vec<u16>, character: u16) -> io::Result<()> {
        // A low surrogate completes the already accepted high unit. It must
        // not overwrite a second following grapheme in overwrite mode.
        let completes_pair = (0xdc00..=0xdfff).contains(&character)
            && self.cursor > 0
            && (0xd800..=0xdbff).contains(&line[self.cursor - 1]);
        let end = if !self.insert && !completes_pair {
            next(line, self.cursor)
        } else {
            self.cursor
        };
        let length = line.len() - (end - self.cursor) + 1;
        if length > MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "console input line budget exhausted",
            ));
        }
        line.splice(self.cursor..end, [character]);
        self.cursor += 1;
        Ok(())
    }
}

// Lossy text is used only to find grapheme boundaries. Each invalid UTF-16
// unit maps to one replacement unit; the stored API result is never normalized.
fn previous(line: &[u16], cursor: usize) -> usize {
    let text = String::from_utf16_lossy(&line[..cursor]);
    cursor
        - text
            .graphemes(true)
            .next_back()
            .map_or(0, |s| s.encode_utf16().count())
}

fn next(line: &[u16], cursor: usize) -> usize {
    let text = String::from_utf16_lossy(&line[cursor..]);
    cursor
        + text
            .graphemes(true)
            .next()
            .map_or(0, |s| s.encode_utf16().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_cooked_editor_bounds_growth_without_rejecting_replacement() {
        let mut line = vec![97; MAX_LINE];
        let mut editor = Editor::new(line.len(), true);
        assert_eq!(
            editor
                .handle(&mut line, 88, 0, 0, true, 0)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(line, vec![97; MAX_LINE]);
        assert_eq!(editor.cursor, MAX_LINE);
        editor.handle(&mut line, 0, 0x24, 0, true, 0).unwrap();
        editor.handle(&mut line, 0, 0x2d, 0, true, 0).unwrap();
        editor.handle(&mut line, 88, 0, 0, true, 0).unwrap();
        assert_eq!(line.len(), MAX_LINE);
        assert_eq!(line[0], 88);
        assert_eq!(line[1..], vec![97; MAX_LINE - 1]);
        editor.handle(&mut line, 13, 13, 0, true, 0).unwrap();
        assert_eq!(&line[MAX_LINE..], &[13, 10]);
    }
}
